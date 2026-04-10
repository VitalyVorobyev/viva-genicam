use std::fs::File;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::convert::TryInto;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use viva_genapi_xml::{self, XmlError};
use viva_genicam::genapi::NodeMap;
use viva_genicam::{Camera, GigeRegisterIo};
use viva_gige::DeviceInfo;
use viva_gige::discover_on_interface;
use viva_gige::gvcp::GigeDevice;
use viva_gige::nic::Iface;
use viva_gige::{GVCP_PORT, discover};

pub const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 500;

pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub async fn discover_devices(
    timeout: Duration,
    iface_ip: Option<Ipv4Addr>,
) -> Result<Vec<DeviceInfo>> {
    let devices = if let Some(ip) = iface_ip {
        let iface = Iface::from_ipv4(ip).context("resolve interface from IPv4 address")?;
        discover_on_interface(timeout, iface.name())
            .await
            .context("discover devices on interface")?
    } else {
        discover(timeout).await.context("broadcast discovery")?
    };
    Ok(devices)
}

pub async fn select_device(
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    iface_ip: Option<Ipv4Addr>,
    timeout: Duration,
) -> Result<DeviceInfo> {
    match (ip, index) {
        (Some(ip), None) => {
            let mut devices = discover_devices(timeout, iface_ip).await?;
            if let Some(found) = devices.drain(..).find(|dev| dev.ip == ip) {
                return Ok(found);
            }
            Ok(DeviceInfo {
                ip,
                mac: [0; 6],
                manufacturer: None,
                model: None,
            })
        }
        (None, Some(idx)) => {
            let devices = discover_devices(timeout, iface_ip).await?;
            let device = devices
                .into_iter()
                .nth(idx)
                .ok_or_else(|| anyhow!("no device at index {idx}"))?;
            Ok(device)
        }
        (Some(ip), Some(_)) => {
            bail!("specify either --ip or --index, not both (using {ip})");
        }
        (None, None) => {
            bail!("a camera must be selected via --ip or --index");
        }
    }
}

async fn fetch_xml(control: Arc<Mutex<GigeDevice>>) -> Result<String> {
    viva_genapi_xml::fetch_and_load_xml({
        move |address, length| {
            let control = Arc::clone(&control);
            async move {
                let mut guard = control.lock().await;
                guard
                    .read_mem(address, length)
                    .await
                    .map_err(|err| XmlError::Transport(err.to_string()))
            }
        }
    })
    .await
    .context("fetch GenApi XML")
}

pub async fn open_camera(device: &DeviceInfo) -> Result<Camera<GigeRegisterIo>> {
    let addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    let control =
        Arc::new(Mutex::new(GigeDevice::open(addr).await.with_context(
            || format!("connect GVCP control channel at {}", device.ip),
        )?));
    let xml = fetch_xml(control.clone()).await?;
    let model = viva_genapi_xml::parse(&xml).context("parse GenApi XML")?;
    let nodemap = NodeMap::from(model);
    let handle = Handle::current();
    let device = Arc::try_unwrap(control)
        .map_err(|_| anyhow!("control connection still in use"))?
        .into_inner();
    let transport = GigeRegisterIo::new(handle, device);
    Ok(Camera::new(transport, nodemap))
}

pub async fn open_stream_device(device: &DeviceInfo) -> Result<GigeDevice> {
    let addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    GigeDevice::open(addr)
        .await
        .with_context(|| format!("open GVCP stream control at {}", device.ip))
}

pub fn resolve_iface(ip: Option<Ipv4Addr>) -> Result<Option<Iface>> {
    if let Some(ip) = ip {
        let iface = Iface::from_ipv4(ip).context("resolve interface from IPv4 address")?;
        Ok(Some(iface))
    } else {
        Ok(None)
    }
}

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("serialise JSON output")?;
    println!("{text}");
    Ok(())
}

pub fn format_system_time(ts: SystemTime) -> Result<String> {
    let dt: OffsetDateTime = <SystemTime as std::convert::Into<OffsetDateTime>>::into(ts);
    dt.format(&Rfc3339).context("format timestamp")
}

pub fn encode_pgm(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    // Lossless, portable conversions (works on any pointer width)
    let w: usize = width.try_into().context("width doesn't fit in usize")?;
    let h: usize = height.try_into().context("height doesn't fit in usize")?;

    // Guard against overflow in w * h
    let expected = w.checked_mul(h).context("image area overflow")?;

    if expected != data.len() {
        bail!(
            "PGM payload length mismatch: expected {expected}, got {}",
            data.len()
        );
    }

    let header = format!("P5\n{width} {height}\n255\n");
    let mut buf = Vec::with_capacity(header.len() + data.len());
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(data);
    Ok(buf)
}

pub fn encode_ppm(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    let w: usize = width.try_into().context("width doesn't fit in usize")?;
    let h: usize = height.try_into().context("height doesn't fit in usize")?;

    // Guard against overflow in w * h * 3 (RGB)
    let expected = w
        .checked_mul(h)
        .and_then(|px| px.checked_mul(3))
        .context("image area overflow")?;

    if expected != data.len() {
        bail!(
            "PPM payload length mismatch: expected {expected}, got {}",
            data.len()
        );
    }
    let header = format!("P6\n{width} {height}\n255\n");
    let mut buf = Vec::with_capacity(header.len() + data.len());
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(data);
    Ok(buf)
}

pub fn save_image(buffer: &[u8], path: &PathBuf) -> Result<()> {
    let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(buffer)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgm_header_is_correct() {
        let data = vec![0u8; 4];
        let encoded = encode_pgm(2, 2, &data).expect("encode");
        assert!(encoded.starts_with(b"P5\n2 2\n255\n"));
        assert_eq!(encoded.len(), 4 + "P5\n2 2\n255\n".len());
    }

    #[test]
    fn ppm_header_is_correct() {
        let data = vec![0u8; 12];
        let encoded = encode_ppm(2, 2, &data).expect("encode");
        assert!(encoded.starts_with(b"P6\n2 2\n255\n"));
        assert_eq!(encoded.len(), 12 + "P6\n2 2\n255\n".len());
    }
}
