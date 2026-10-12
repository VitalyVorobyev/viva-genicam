//! The GigE Vision side of the service: a served camera, and how cameras are
//! found.

use std::net::Ipv4Addr;
use std::time::Duration;

use tracing::{debug, warn};
use viva_genicam::gige::{self, Iface, IfaceSelector};
use viva_genicam::report::ReportTarget;
use viva_genicam::{GenicamError, GigeRegisterIo, StreamOptions, connect_gige_with_xml};
use viva_zenoh_api::DeviceAnnounce;

use crate::device::{DeviceOps, SharedCamera};
use crate::discovery::Discovery;
use crate::source::{BlockStream, StreamSource};

/// A GigE Vision camera the service serves.
pub struct DeviceHandle {
    camera: SharedCamera<GigeRegisterIo>,
    raw_xml: String,
    device_id: String,
    info: gige::DeviceInfo,
    /// Host interface for stream setup, already resolved.
    ///
    /// Held resolved rather than as a name: the round trip through a string
    /// is what let the receive path drift into resolving it a second way, and
    /// wrongly (backlog `SVC-06`).
    iface: Option<Iface>,
}

impl DeviceHandle {
    /// Connect to a discovered camera, taking control of it.
    pub async fn connect(
        info: &gige::DeviceInfo,
        iface: Option<Iface>,
    ) -> Result<Self, GenicamError> {
        let (camera, xml) = connect_gige_with_xml(info).await?;
        Ok(Self {
            camera: SharedCamera::new(camera),
            raw_xml: xml,
            device_id: derive_device_id(info),
            info: info.clone(),
            iface,
        })
    }

    /// The camera's Discovery ACK.
    pub fn info(&self) -> &gige::DeviceInfo {
        &self.info
    }

    /// The host interface `--iface` selected, if the operator named one.
    pub fn iface(&self) -> Option<&Iface> {
        self.iface.as_ref()
    }

    /// Refresh the control connection and replace the cached camera handle.
    ///
    /// The Aravis fake camera on macOS loopback can stop producing frames
    /// after a longer idle period even though register reads still succeed.
    /// Reopening the control connection immediately before stream setup
    /// restores the working state without changing the higher-level device
    /// identity.
    ///
    /// The keepalive needs no coordination here. It belongs to the transport,
    /// so the new connection brings its own and the old one retires when the
    /// swap below drops the camera that owned it — nothing has to be paused,
    /// and nothing can outlive the privilege it was protecting.
    pub async fn refresh_connection(&self) -> Result<(), GenicamError> {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY: Duration = Duration::from_millis(500);
        const MAX_DELAY: Duration = Duration::from_secs(16);

        // 1. Give control up first: the old connection holds it until the
        //    swap below drops it, and a camera refuses a second application's
        //    claim while another holds control.
        if let Err(error) = self
            .camera
            .with(|camera| camera.transport().release_control())
            .await
        {
            debug!(device_id = self.device_id, %error, "releasing control before reconnecting");
        }

        // 2. Retry connection with exponential backoff.
        let mut attempt = 0u32;
        let (camera, _xml) = loop {
            attempt += 1;
            match connect_gige_with_xml(&self.info).await {
                Ok(pair) => break pair,
                Err(e) if attempt >= MAX_RETRIES => {
                    warn!(device_id = self.device_id, error = %e, attempt, "reconnect failed, giving up");
                    return Err(e);
                }
                Err(e) => {
                    let delay = BASE_DELAY
                        .saturating_mul(1 << (attempt - 1).min(5))
                        .min(MAX_DELAY);
                    warn!(
                        device_id = self.device_id,
                        error = %e,
                        attempt,
                        ?delay,
                        "reconnect failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        };

        // 3. Swap the camera handle. Dropping the old one retires its keepalive.
        self.camera.replace(camera)
    }
}

#[async_trait::async_trait]
impl DeviceOps for DeviceHandle {
    type Transport = GigeRegisterIo;

    fn device_id(&self) -> &str {
        &self.device_id
    }

    fn raw_xml(&self) -> &str {
        &self.raw_xml
    }

    fn camera(&self) -> &SharedCamera<GigeRegisterIo> {
        &self.camera
    }
}

#[async_trait::async_trait]
impl StreamSource for DeviceHandle {
    /// A GVSP stream through the camera's own control channel
    /// ([`Camera::open_stream`](viva_genicam::Camera::open_stream)): stream
    /// channel, packet size, socket, and a receive engine sized by the
    /// camera's `PayloadSize`. `packet_size` is a ceiling for the path probe;
    /// `None` negotiates one from the NIC's MTU.
    async fn open_stream(
        &self,
        packet_size: Option<u32>,
    ) -> Result<Box<dyn BlockStream>, GenicamError> {
        // Best-effort refresh before stream setup. On macOS loopback the fake
        // Aravis camera can sit discovered/idle for a while and then fail to
        // deliver GVSP until the control channel is reopened. Immediate-start
        // cases can still work with the existing control channel, so a
        // refresh timeout falls back to the current connection instead of
        // failing start.
        if cfg!(target_os = "macos")
            && let Err(e) = self.refresh_connection().await
        {
            warn!(
                device_id = self.device_id,
                error = %e,
                "camera reconnect before acquisition failed; continuing with existing control connection"
            );
        }
        let iface = receive_iface(self.iface(), self.info.ip)?;
        let stream = self
            .camera
            .with(move |camera| {
                let options = StreamOptions::new().iface(iface);
                let options = match packet_size {
                    Some(size) => options.packet_size(size),
                    None => options.auto_packet_size(),
                };
                camera.open_stream(options)
            })
            .await?;
        Ok(Box::new(stream))
    }

    /// The report opens the camera itself, read-only, by its address.
    async fn report_target(&self) -> ReportTarget {
        ReportTarget::Ip(self.info.ip)
    }

    fn service_name(&self) -> &'static str {
        "viva-service"
    }
}

/// The host interface that will receive the camera's GVSP traffic.
///
/// With `--iface` the operator has already chosen, and the handle carries the
/// interface they chose, resolved once.
///
/// Without it, the OS is asked which local interface routes to the camera.
/// This arm used to call `Iface::from_ipv4(device.info().ip)` — the camera's
/// address handed to the *host*-address lookup, which can only ever fail with
/// `no interface with IPv4 <camera-ip>`. So `viva-service` could not stream
/// without `--iface` at all. That is the same confusion as #70 (`REL-04` in
/// the studio backend, `DX-08` in `viva-camctl`); this was the third copy and
/// the last one left (backlog `SVC-06`). Separated so it can be tested
/// without a camera — which matters here, because the fake camera cannot
/// reproduce the defect this guards (see the test).
fn receive_iface(chosen: Option<&Iface>, camera: Ipv4Addr) -> Result<Iface, GenicamError> {
    match chosen {
        Some(iface) => Ok(iface.clone()),
        None => Iface::from_remote_ipv4(camera).map_err(|source| {
            warn!(%camera, "no route to the camera; pass --iface <HOST-IP|NAME> to choose one");
            GenicamError::NoRoute { camera, source }
        }),
    }
}

/// GigE Vision discovery: a broadcast on every interface, or on the one
/// `--iface` names.
pub struct GigeDiscovery {
    timeout: Duration,
    iface: Option<IfaceSelector>,
}

impl GigeDiscovery {
    /// Discover for `timeout` on each pass, on `iface` if given.
    pub fn new(timeout: Duration, iface: Option<IfaceSelector>) -> Self {
        Self { timeout, iface }
    }
}

/// A camera a GigE Vision discovery pass found, with the interface it is
/// served on.
#[derive(Debug, Clone)]
pub struct GigeFound {
    /// Its Discovery ACK.
    pub info: gige::DeviceInfo,
    /// The interface `--iface` resolved to, on this pass.
    pub iface: Option<Iface>,
}

#[async_trait::async_trait]
impl Discovery for GigeDiscovery {
    type Found = GigeFound;
    type Device = DeviceHandle;

    /// Resolves `--iface` on every pass rather than once at startup, so an
    /// interface that comes up after the service does is still picked up. A
    /// selector that resolves to nothing is a failed pass — retried next
    /// tick, not fatal, and not a broadcast scan: the operator named an
    /// interface, and scanning every other one instead would be answering a
    /// question they did not ask. The error carries the interfaces this host
    /// reports, which is what a Windows operator needs to find the GUID.
    async fn discover(&self) -> Result<Vec<GigeFound>, GenicamError> {
        let iface = self
            .iface
            .as_ref()
            .map(IfaceSelector::resolve)
            .transpose()
            .map_err(|source| GenicamError::Network {
                during: "resolve --iface",
                source,
            })?;
        let found = match &iface {
            Some(iface) => gige::discover_on_interface(self.timeout, iface.name()).await,
            None => gige::discover(self.timeout).await,
        }
        .map_err(|source| GenicamError::Gige {
            during: "discovery",
            source,
        })?;
        Ok(found
            .into_iter()
            .map(|info| GigeFound {
                info,
                iface: iface.clone(),
            })
            .collect())
    }

    fn announce(&self, found: &GigeFound) -> DeviceAnnounce {
        device_announce(&found.info)
    }

    async fn connect(&self, found: &GigeFound) -> Result<DeviceHandle, GenicamError> {
        DeviceHandle::connect(&found.info, found.iface.clone()).await
    }
}

/// Build the announce for a discovered camera from its Discovery ACK.
///
/// `serial` is the serial the camera reports. It used to be the device id,
/// which left two cameras of one model with nothing to tell them apart but a
/// MAC-derived string no client rendered (#137).
pub fn device_announce(info: &gige::DeviceInfo) -> DeviceAnnounce {
    DeviceAnnounce::for_gige(
        &info.mac,
        info.ip,
        info.model.as_deref(),
        info.serial.as_deref(),
        info.user_name.as_deref(),
        info.manufacturer.as_deref(),
    )
}

/// The id a GigE Vision camera is published under: the `id` of
/// [`device_announce`].
pub fn derive_device_id(info: &gige::DeviceInfo) -> String {
    viva_zenoh_api::gige_device_id(&info.mac)
}

/// Announce a discovered camera on `keys::announce`.
pub async fn publish_announce(session: &zenoh::Session, info: &gige::DeviceInfo) {
    crate::serve::put_announce(session, &device_announce(info)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(serial: Option<&str>, user_name: Option<&str>) -> gige::DeviceInfo {
        gige::DeviceInfo {
            mac: [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
            model: Some("FakeGigE".into()),
            manufacturer: Some("viva-genicam".into()),
            serial: serial.map(Into::into),
            user_name: user_name.map(Into::into),
            ..gige::DeviceInfo::from_ip(Ipv4Addr::new(192, 168, 1, 10))
        }
    }

    #[test]
    fn announce_carries_the_device_identity_not_the_device_id() {
        let info = discovered(Some("SN-1"), Some("Left"));
        let a = device_announce(&info);
        assert_eq!(a.id, "cam-deadbeefcafe");
        assert_eq!(a.id, derive_device_id(&info));
        assert_eq!(a.serial, "SN-1");
        assert_eq!(a.name, "Left");
        assert_eq!(a.model, "FakeGigE");
        assert_eq!(a.ip.as_deref(), Some("192.168.1.10"));
        assert_eq!(a.mac.as_deref(), Some("DE:AD:BE:EF:CA:FE"));
        assert_eq!(a.user_name.as_deref(), Some("Left"));
        assert_eq!(a.manufacturer.as_deref(), Some("viva-genicam"));
        assert_eq!(a.api_version, Some(viva_zenoh_api::API_VERSION));
    }

    #[test]
    fn announce_without_serial_or_user_name_leaves_them_empty() {
        let info = discovered(None, None);
        let a = device_announce(&info);
        assert_eq!(a.serial, "", "the device id must not stand in for a serial");
        assert_eq!(a.user_name, None);
        assert_eq!(a.name, "FakeGigE");
    }

    /// Backlog `SVC-06`. Without `--iface` the receive interface must come
    /// from a route probe, not from looking the *camera's* address up among
    /// the host's own — a lookup that can only ever fail.
    ///
    /// `127.0.0.2` is the whole test: it is routable (loopback answers for the
    /// entire `127/8`) but it is not an address any interface reports, so the
    /// two functions give visibly different answers. The camera we test
    /// against cannot show this, because it lives on `127.0.0.1`, which *is* a
    /// host address — so the broken lookup succeeds there by coincidence. That
    /// is why this is a unit test and not an e2e one.
    #[test]
    fn no_iface_probes_the_route_rather_than_looking_up_the_camera_address() {
        let camera_ip = Ipv4Addr::new(127, 0, 0, 2);

        // What the code used to do.
        assert!(
            Iface::from_ipv4(camera_ip).is_err(),
            "a camera address is not a host address; if this ever succeeds \
             the test has stopped proving anything"
        );

        // What it does now.
        let iface = receive_iface(None, camera_ip).expect("route probe should resolve");
        assert_eq!(iface.ipv4(), Some(Ipv4Addr::LOCALHOST));
    }

    /// An explicit `--iface` is honoured verbatim; the camera's address is not
    /// consulted at all.
    #[test]
    fn an_explicit_iface_is_used_as_given() {
        let chosen = Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback iface");
        let resolved = receive_iface(Some(&chosen), Ipv4Addr::new(192, 0, 2, 1))
            .expect("an explicit interface needs no probe");
        assert_eq!(resolved, chosen);
    }
}
