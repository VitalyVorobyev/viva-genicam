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
use tokio::sync::Mutex;
use viva_genapi_xml::{self, XmlError};
use viva_genicam::image_io::{self, ImageFormat};
use viva_genicam::pfnc::{self, PixelFormat};
use viva_genicam::{Access, Camera, GigeRegisterIo, U3vRegisterIo, connect_gige_as};
use viva_gige::DeviceInfo;
use viva_gige::GigeDevice;
use viva_gige::discover_on_interface;
use viva_gige::{GVCP_PORT, discover};
use viva_gige::{Iface, IfaceSelector};
use viva_u3v::NusbTransfer;

pub const DEFAULT_DISCOVERY_TIMEOUT_MS: u64 = 500;

/// The camera a command acts on: a GigE Vision camera by address or by its
/// place in discovery order, or a USB3 Vision camera by its place in
/// `list-usb` order.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct Target {
    /// A GigE Vision camera, by IP address
    #[arg(long, conflicts_with = "usb")]
    pub ip: Option<Ipv4Addr>,
    /// A GigE Vision camera, by its place in discovery order
    #[arg(long, conflicts_with = "usb")]
    pub index: Option<usize>,
    /// A USB3 Vision camera, by its place in `list-usb` order
    #[arg(long)]
    pub usb: Option<usize>,
}

/// A camera opened through either transport. Commands that need only what
/// [`Camera`] offers on any transport take one and dispatch with
/// [`with_camera!`](crate::with_camera).
pub enum AnyCamera {
    /// GigE Vision.
    Gige(Camera<GigeRegisterIo>),
    /// USB3 Vision.
    Usb(Camera<U3vRegisterIo<NusbTransfer>>),
}

/// Run `$body` with `$camera` bound to the camera inside an
/// [`AnyCamera`](crate::common::AnyCamera), whichever transport it is on.
macro_rules! with_camera {
    ($any:expr, $camera:ident => $body:expr) => {
        match $any {
            $crate::common::AnyCamera::Gige($camera) => $body,
            $crate::common::AnyCamera::Usb($camera) => $body,
        }
    };
}
pub(crate) use with_camera;

/// Open the camera `target` names, with its nodemap. `access` applies to a
/// GigE Vision camera; USB3 Vision has no control privilege.
pub async fn open_target(
    target: &Target,
    iface: Option<&IfaceSelector>,
    access: Access,
) -> Result<AnyCamera> {
    if let Some(usb) = target.usb {
        return Ok(AnyCamera::Usb(crate::cmd_usb::open_usb_camera(Some(usb))?));
    }
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = select_device(target.ip, target.index, iface, timeout).await?;
    Ok(AnyCamera::Gige(open_camera(&device, access).await?))
}

/// A number as decimal or `0x` hexadecimal, for addresses and register
/// values.
pub fn parse_number(text: &str) -> Result<u64, String> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(&hex.replace('_', ""), 16),
        None => text.replace('_', "").parse(),
    }
    .map_err(|err| format!("{text:?} is not a decimal or 0x-hexadecimal number: {err}"))
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub async fn discover_devices(
    timeout: Duration,
    iface: Option<&IfaceSelector>,
) -> Result<Vec<DeviceInfo>> {
    let devices = if let Some(selector) = iface {
        let iface = resolve_iface_selector(selector)?;
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
    iface: Option<&IfaceSelector>,
    timeout: Duration,
) -> Result<DeviceInfo> {
    match (ip, index) {
        (Some(ip), None) => {
            let mut devices = discover_devices(timeout, iface).await?;
            if let Some(found) = devices.drain(..).find(|dev| dev.ip == ip) {
                return Ok(found);
            }
            Ok(DeviceInfo::from_ip(ip))
        }
        (None, Some(idx)) => {
            let devices = discover_devices(timeout, iface).await?;
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

/// Fetch a camera's GenApi XML, stopping before anything is made of it.
///
/// Deliberately separate from [`open_camera`]: the cameras whose XML we most
/// want are the ones whose XML we cannot yet parse, so the dump must not
/// depend on parsing succeeding.
pub async fn fetch_xml(control: Arc<Mutex<GigeDevice>>) -> Result<String> {
    viva_genapi_xml::fetch_and_load_xml({
        move |address, length| {
            let control = Arc::clone(&control);
            async move {
                let mut guard = control.lock().await;
                guard
                    .read_mem(address, length)
                    .await
                    .map_err(XmlError::transport)
            }
        }
    })
    .await
    .context("fetch GenApi XML")
}

/// Open a camera with its nodemap: as the controller for a command that
/// writes, as a read-only monitor for one that only reads — which then works
/// while another application controls the camera.
pub async fn open_camera(device: &DeviceInfo, access: Access) -> Result<Camera<GigeRegisterIo>> {
    let (camera, _xml) = connect_gige_as(device, access)
        .await
        .with_context(|| format!("open the camera at {}", device.ip))?;
    Ok(camera)
}

/// Open the GVCP control channel and stop there.
///
/// Streaming, IP configuration and the XML dump all start here; none of them
/// needs a nodemap, and one of them exists precisely because building a
/// nodemap can fail.
pub async fn open_control(device: &DeviceInfo) -> Result<GigeDevice> {
    let addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    GigeDevice::open(addr)
        .await
        .with_context(|| format!("connect GVCP control channel at {}", device.ip))
}

/// Resolve `--iface` against the host, whichever way the user spelled it.
///
/// The spelling is [`IfaceSelector`]'s problem, not this crate's: an IPv4
/// address and an OS interface name are both accepted here, in `viva-service`
/// and in the Python bindings, because a user who found a camera with one tool
/// should be able to stream it with the next
/// ([#109](https://github.com/VitalyVorobyev/viva-genicam/issues/109)).
pub fn resolve_iface_selector(selector: &IfaceSelector) -> Result<Iface> {
    selector
        .resolve()
        .with_context(|| format!("resolve host interface '{selector}'"))
}

/// Resolve the local interface that will receive packets from `camera_ip`.
///
/// Without `--iface`, ask the OS which local interface routes to the camera
/// instead of refusing to run: `list`, `xml`, `report`, `get`, `set` and
/// `set-ip` all tolerate a missing `--iface` and fall back to broadcast
/// discovery, and `stream`, `bench` and `events` were the exceptions.
///
/// That mattered beyond consistency. `viva-camctl stream --ip <IP>` is the
/// command our own documentation hands to anyone reporting a camera we cannot
/// open, and it exited before touching the network. Meanwhile
/// [`Iface::from_remote_ipv4`] — the route probe added by #72 *for this exact
/// case*, and produced by that very issue — had no caller in this crate
/// (backlog `DX-08`).
///
/// Note which side of the split each function serves: the selector names a
/// **host** interface, while `camera_ip` is **remote**, so the fallback must
/// be `from_remote_ipv4` and never `from_ipv4`. Passing a camera address to
/// `from_ipv4` is the #70 defect, and `viva-service` still had a copy of it
/// (backlog `SVC-06`).
pub fn resolve_receive_iface(iface: Option<&IfaceSelector>, camera_ip: Ipv4Addr) -> Result<Iface> {
    match iface {
        Some(selector) => resolve_iface_selector(selector),
        None => Iface::from_remote_ipv4(camera_ip).with_context(|| {
            format!(
                "probe which local interface routes to {camera_ip} \
                 (pass --iface <HOST-IP|NAME> to choose one explicitly)"
            )
        }),
    }
}

pub fn resolve_iface(iface: Option<&IfaceSelector>) -> Result<Option<Iface>> {
    iface.map(resolve_iface_selector).transpose()
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

/// The file format `--save-format` writes frames in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SaveFormat {
    /// 8-bit NetPBM: PGM for Mono8, PPM for anything else or with `--rgb`.
    Pnm,
    /// PNG at the camera's bit depth: 16-bit when samples are wider than 8
    /// bits, with the depth in `sBIT`.
    Png,
    /// Uncompressed TIFF at the camera's bit depth.
    Tiff,
}

/// A frame's pixels as a file in `save`, and the extension to give it.
/// `rgb` writes colour even for a monochrome frame; `row_stride` is the
/// frame's, for a camera that pads its lines.
pub fn encode_frame(
    format: PixelFormat,
    width: u32,
    height: u32,
    row_stride: Option<usize>,
    payload: &[u8],
    save: SaveFormat,
    rgb: bool,
) -> Result<(Vec<u8>, &'static str)> {
    let file = match save {
        SaveFormat::Pnm if !rgb && format == PixelFormat::Mono8 => {
            let mut converter = pfnc::convert::Converter::new();
            let pixels = converter
                .gray8(format, width, height, row_stride, payload)
                .map_err(|e| anyhow!("convert frame: {e}"))?;
            return Ok((encode_pgm(width, height, pixels)?, "pgm"));
        }
        SaveFormat::Pnm => {
            let pixels = pfnc::convert::to_rgb8(format, width, height, row_stride, payload)
                .map_err(|e| anyhow!("convert frame to RGB8: {e}"))?;
            return Ok((encode_ppm(width, height, &pixels)?, "ppm"));
        }
        SaveFormat::Png => ImageFormat::Png,
        SaveFormat::Tiff => ImageFormat::Tiff,
    };
    let pixels = pfnc::convert::full_depth(format, width, height, row_stride, payload)
        .map_err(|e| anyhow!("convert frame: {e}"))?;
    let pixels = if rgb { pixels.into_rgb() } else { pixels };
    let encoded = image_io::encode(&pixels, width, height, file)
        .with_context(|| format!("encode {}", file.extension()))?;
    Ok((encoded, file.extension()))
}

pub fn save_image(buffer: &[u8], path: &PathBuf) -> Result<()> {
    let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(buffer)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// One line of a stream's counters, as the receive engine keeps them: every
/// block the camera sent is complete, incomplete, missed or dropped to
/// backpressure.
pub fn print_stream_counters(prefix: &str, stats: &viva_genicam::StreamStats) {
    println!(
        "{prefix} fps={:.1} Mbps={:.2} complete={} incomplete={} missed={} backpressure={} \
         resend={}/{}/{} bytes={}",
        stats.avg_fps,
        stats.avg_mbps,
        stats.frames_complete,
        stats.frames_incomplete,
        stats.frames_missed,
        stats.frames_backpressure,
        stats.resend_requested,
        stats.resend_recovered,
        stats.resend_unavailable,
        stats.bytes,
    );
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

    /// Mono8 stays PGM by default; PNG and TIFF keep a 12-bit camera's depth.
    #[test]
    fn a_frame_is_saved_in_the_format_asked_for() {
        let mono8 =
            |save, rgb| encode_frame(PixelFormat::Mono8, 1, 1, None, &[7], save, rgb).unwrap();
        let (pgm, ext) = mono8(SaveFormat::Pnm, false);
        assert_eq!((pgm.as_slice(), ext), (&b"P5\n1 1\n255\n\x07"[..], "pgm"));
        let (ppm, ext) = mono8(SaveFormat::Pnm, true);
        assert_eq!(
            (ppm.as_slice(), ext),
            (&b"P6\n1 1\n255\n\x07\x07\x07"[..], "ppm")
        );

        let mono12 = [0xBC, 0x0A];
        let (png, ext) = encode_frame(
            PixelFormat::Mono12,
            1,
            1,
            None,
            &mono12,
            SaveFormat::Png,
            false,
        )
        .unwrap();
        assert_eq!(ext, "png");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // IHDR: bit depth 16, colour type 0 (greyscale).
        assert_eq!(&png[24..26], [16, 0]);
        let (png, _) = encode_frame(
            PixelFormat::Mono12,
            1,
            1,
            None,
            &mono12,
            SaveFormat::Png,
            true,
        )
        .unwrap();
        // Colour type 2 (truecolour).
        assert_eq!(&png[24..26], [16, 2]);

        let (tiff, ext) = encode_frame(
            PixelFormat::Mono12,
            1,
            1,
            None,
            &mono12,
            SaveFormat::Tiff,
            false,
        )
        .unwrap();
        assert_eq!(ext, "tiff");
        assert!(tiff.starts_with(b"II*\0") || tiff.starts_with(b"MM\0*"));
    }

    #[test]
    fn ppm_header_is_correct() {
        let data = vec![0u8; 12];
        let encoded = encode_ppm(2, 2, &data).expect("encode");
        assert!(encoded.starts_with(b"P6\n2 2\n255\n"));
        assert_eq!(encoded.len(), 12 + "P6\n2 2\n255\n".len());
    }
}
