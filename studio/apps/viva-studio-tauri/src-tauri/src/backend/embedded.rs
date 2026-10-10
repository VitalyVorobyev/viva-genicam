//! Embedded device backend: direct GigE Vision and USB3 Vision camera access.
//!
//! This backend uses `viva-genicam` to communicate directly with cameras without
//! requiring an external service process. It is the default mode when no Zenoh
//! configuration is detected.
//!
//! ## Thread safety
//!
//! `Camera<GigeRegisterIo>` is `!Sync`: its `NodeMap` caches in `RefCell`s, and
//! its register I/O blocks the calling thread on the runtime. So the camera
//! lives behind an async mutex, and every operation on it runs on the blocking
//! pool ([`EmbeddedBackend::with_camera`]): a command waiting for the camera
//! waits asynchronously, and camera I/O never occupies a runtime worker
//! (backlog ST-31).

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::{Mutex as AsyncMutex, RwLock, watch};
use tracing::{debug, info, warn};
use viva_genicam::{Camera, FrameStream, GigeRegisterIo};
use viva_streamer::bmp::BayerPattern;
use viva_streamer::frame::{FrameEncoder, FramePacer, PixelLayout};
use viva_zenoh_api::{AcquisitionStatus, DeviceAnnounce, FeatureState};

use crate::state::device_state::{DeviceInfo, StreamerInfo};
use crate::state::events::Events;

use super::{BackendMode, ConnectResult, DeviceBackend, NetworkConfig, StartOptions};

// ── Internal types ──────────────────────────────────────────────────────────

/// A connected GigE Vision camera and its associated state.
///
/// `Send` is derived, not asserted: every field is `Send` already. The
/// `unsafe impl Send` this type used to carry was redundant, and a redundant
/// unsafe impl is worse than none — it would have gone on silently covering
/// for a genuinely non-`Send` field added later.
struct ConnectedCamera {
    camera: Camera<GigeRegisterIo>,
    /// The id discovery reported, to find the camera's MAC again.
    device_id: String,
}

impl ConnectedCamera {
    /// Snapshot one feature of this camera, computed by `viva-genapi` exactly
    /// as `viva-service` computes it (backlog ST-26).
    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        self.camera
            .feature_state(name)
            .map(FeatureState::from)
            .map_err(|e| format!("Failed to read '{name}': {e}"))
    }
}

/// State for an active image acquisition session.
struct AcquisitionState {
    shutdown_tx: watch::Sender<bool>,
    task_handles: Vec<tauri::async_runtime::JoinHandle<()>>,
}

// ── EmbeddedBackend ─────────────────────────────────────────────────────────

/// How long each step of releasing a camera waits for its acknowledgement.
/// Short, because the camera may already be gone and app exit waits on it.
const RELEASE_ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// The most frames a second the viewer is sent. A camera can deliver far
/// more, and encoding each as a BMP costs CPU the user cannot see.
const DISPLAY_FPS: u32 = 30;

/// How often a stream reports its frame rate and dropped frames.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Backend that communicates directly with cameras via GigE Vision / USB3 Vision.
pub struct EmbeddedBackend {
    /// The connected camera; see the module docs.
    camera: Arc<AsyncMutex<Option<ConnectedCamera>>>,
    /// Cache of discovered devices, updated periodically.
    discovered: RwLock<Vec<DeviceInfo>>,
    /// The host interface each discovered camera answered on, by MAC.
    interfaces: RwLock<HashMap<[u8; 6], String>>,
    /// Active acquisition state, if streaming.
    acquisition: AsyncMutex<Option<AcquisitionState>>,
    events: Arc<Events>,
}

impl EmbeddedBackend {
    /// Create a new embedded backend with no connected camera.
    pub fn new(events: Arc<Events>) -> Self {
        Self {
            camera: Arc::new(AsyncMutex::new(None)),
            discovered: RwLock::new(Vec::new()),
            interfaces: RwLock::new(HashMap::new()),
            acquisition: AsyncMutex::new(None),
            events,
        }
    }

    /// Start a background task that periodically discovers GigE cameras.
    pub fn start_discovery_task(self: &Arc<Self>, interval: Duration) {
        let backend = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let (devices, interfaces) = discover_gige_devices().await;
                *backend.discovered.write().await = devices.clone();
                *backend.interfaces.write().await = interfaces;
                for d in &devices {
                    backend.events.send(d);
                }
            }
        });
    }

    /// Run `op` on the connected camera, on the blocking pool.
    ///
    /// The camera's register I/O blocks its thread, so it must not run on a
    /// runtime worker; and waiting for another operation to finish with the
    /// camera must not hold one either. This replaced `block_in_place` under
    /// a `std::sync::Mutex`, which did both (backlog ST-31).
    async fn with_camera<R, F>(&self, op: F) -> Result<R, String>
    where
        F: FnOnce(&mut ConnectedCamera) -> Result<R, String> + Send + 'static,
        R: Send + 'static,
    {
        let mut slot = self.camera.clone().lock_owned().await;
        tauri::async_runtime::spawn_blocking(move || {
            let connected = slot
                .as_mut()
                .ok_or_else(|| "No camera connected".to_string())?;
            op(connected)
        })
        .await
        .map_err(|e| format!("Camera task failed: {e}"))?
    }

    /// Stop streamer tasks and clean up acquisition state.
    async fn stop_acquisition_inner(&self) {
        let mut acq = self.acquisition.lock().await;
        if let Some(state) = acq.take() {
            let _ = state.shutdown_tx.send(true);
            for handle in state.task_handles {
                handle.abort();
                // Wait for the task to go, so no frame reader outlives the
                // stream it reads; an aborted task resolves as cancelled.
                let _ = handle.await;
            }
        }
    }

    /// Stop streaming, tell the camera to stop acquiring, give back control
    /// privilege, and drop the camera.
    ///
    /// Every step is best-effort: an unplugged camera cannot answer, and the
    /// caller is disconnecting or exiting either way. Dropping the camera joins
    /// its keepalive (`GigeRegisterIo`'s `Drop`), which needs the runtime the
    /// blocking pool belongs to.
    async fn release_camera(&self) {
        self.stop_acquisition_inner().await;
        let Some(mut connected) = self.camera.lock().await.take() else {
            return;
        };
        let released = tauri::async_runtime::spawn_blocking(move || {
            if let Ok(mut device) = connected.camera.transport().lock_device() {
                device.set_control_timeout(RELEASE_ACK_TIMEOUT);
            }
            if let Err(error) = connected.camera.acquisition_stop() {
                debug!(%error, "AcquisitionStop while releasing the camera");
            }
            let handle = tokio::runtime::Handle::current();
            match connected.camera.transport().lock_device() {
                Ok(mut device) => {
                    if let Err(error) = handle.block_on(device.release_control()) {
                        debug!(%error, "releasing control privilege");
                    }
                }
                Err(error) => debug!(%error, "releasing control privilege"),
            }
            drop(connected);
        })
        .await;
        if let Err(error) = released {
            warn!(%error, "releasing the camera failed");
        }
        info!("Camera released (embedded mode)");
    }
}

#[async_trait]
impl DeviceBackend for EmbeddedBackend {
    fn mode(&self) -> BackendMode {
        BackendMode::Embedded
    }

    async fn discover(&self) -> Vec<DeviceInfo> {
        self.discovered.read().await.clone()
    }

    async fn connect(&self, device_id: &str) -> Result<ConnectResult, String> {
        // Disconnect any existing camera first.
        self.release_camera().await;

        // Look up device info from discovery cache. The id is MAC-derived, so
        // the address comes from the cache; a bare IPv4 id is still accepted
        // for a camera discovery has not reported.
        let cached = {
            let discovered = self.discovered.read().await;
            discovered.iter().find(|d| d.id == device_id).cloned()
        };
        let ip: Ipv4Addr = cached
            .as_ref()
            .and_then(|d| d.ip.as_deref())
            .unwrap_or(device_id)
            .parse()
            .map_err(|_| {
                format!(
                    "Unknown device '{device_id}': not in the discovery list, and not an IPv4 address"
                )
            })?;
        let (name, model) = cached
            .map(|d| (d.name, d.model))
            .unwrap_or_else(|| (device_id.to_string(), String::new()));

        let gige_info = viva_genicam::gige::DeviceInfo {
            model: if model.is_empty() {
                None
            } else {
                Some(model.clone())
            },
            ..viva_genicam::gige::DeviceInfo::from_ip(ip)
        };

        info!(device_id, "Connecting to GigE camera (embedded mode)");

        let (camera, xml) = viva_genicam::connect_gige_with_xml(&gige_info)
            .await
            .map_err(|e| format!("Failed to connect to camera at {device_id}: {e}"))?;

        info!(
            device_id,
            xml_len = xml.len(),
            "Camera connected, XML fetched"
        );

        *self.camera.lock().await = Some(ConnectedCamera {
            camera,
            device_id: device_id.to_string(),
        });

        Ok(ConnectResult {
            xml,
            device_name: name,
            model,
        })
    }

    async fn disconnect(&self) -> Result<(), String> {
        self.release_camera().await;
        Ok(())
    }

    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let name = name.to_string();
        self.with_camera(move |connected| connected.feature_state(&name))
            .await
    }

    async fn set_feature(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let value = json_value_to_string(value);
        let name = name.to_string();
        self.with_camera(move |connected| {
            connected
                .camera
                .set(&name, &value)
                .map_err(|e| format!("Failed to write feature '{name}': {e}"))?;
            Ok(connected.camera.nodemap().invalidated_by(&name))
        })
        .await
    }

    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String> {
        let name = name.to_string();
        self.with_camera(move |connected| {
            connected
                .camera
                .set(&name, "")
                .map_err(|e| format!("Failed to execute command '{name}': {e}"))?;
            Ok(connected.camera.nodemap().invalidated_by(&name))
        })
        .await
    }

    async fn bulk_feature_state(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, FeatureState>, String> {
        let names = names.to_vec();
        self.with_camera(move |connected| {
            let mut result = HashMap::with_capacity(names.len());
            for name in names {
                match connected.feature_state(&name) {
                    Ok(state) => {
                        result.insert(name, state);
                    }
                    Err(e) => {
                        warn!(name, error = %e, "Failed to read feature in bulk_feature_state");
                    }
                }
            }
            Ok(result)
        })
        .await
    }

    async fn start_acquisition(&self, options: StartOptions) -> Result<StreamerInfo, String> {
        // Stop any previous acquisition.
        self.stop_acquisition_inner().await;

        let (width, height, pixel_format, frame_stream) = self
            .with_camera(move |connected| {
                let cam = &mut connected.camera;
                let width = cam
                    .get("Width")
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(640);
                let height = cam
                    .get("Height")
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(480);
                let pixel_format = cam.get("PixelFormat").unwrap_or_else(|error| {
                    warn!(%error, "Failed to read PixelFormat before acquisition");
                    "Unknown".to_string()
                });

                let mut device_guard = cam
                    .transport()
                    .lock_device()
                    .map_err(|e| format!("Failed to access device: {e}"))?;

                let handle = tokio::runtime::Handle::current();
                handle
                    .block_on(device_guard.claim_control())
                    .map_err(|e| format!("Failed to claim camera control: {e}"))?;

                let camera_ip = device_guard.remote_addr().ip();
                let camera_ipv4 = match camera_ip {
                    std::net::IpAddr::V4(ip) => ip,
                    _ => return Err("IPv6 cameras are not supported".to_string()),
                };

                // camera_ipv4 belongs to the remote camera, not the host. Probe
                // the OS route to find the local NIC and source address that
                // the OS selected to reach that camera.
                let iface = viva_genicam::gige::nic::Iface::from_remote_ipv4(camera_ipv4)
                    .map_err(|e| format!("Failed to detect network interface: {e}"))?;

                let builder = viva_genicam::StreamBuilder::new(&mut device_guard)
                    .iface(iface)
                    .rcvbuf_bytes(64 << 20); // 64 MiB to absorb bursty GVSP traffic
                // A size the user measured for their link is a ceiling the
                // path probe stays under; otherwise negotiate from the NIC MTU
                // (backlog ST-22, issue #112).
                let builder = match options.packet_size {
                    Some(size) => builder.packet_size(size),
                    None => builder.auto_packet_size(),
                };
                let stream = handle
                    .block_on(builder.build())
                    .map_err(|e| format!("Failed to build stream: {e}"))?;

                drop(device_guard);

                let frame_stream = FrameStream::new(stream, None);

                cam.acquisition_start()
                    .map_err(|e| format!("Failed to start acquisition: {e}"))?;

                Ok((width, height, pixel_format, frame_stream))
            })
            .await?;

        // Create WS broadcast channels.
        let (frame_tx, _) = watch::channel(Bytes::new());
        let info_tx = watch::channel(camera_stream_info(width, height, pixel_format.clone())).0;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Bind WebSocket server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("Failed to bind WS listener: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("Failed to get local address: {e}"))?
            .port();
        let ws_url = format!("ws://127.0.0.1:{port}/ws");

        info!(ws_url, width, height, "Embedded streamer starting");

        let frame_reader_handle = tauri::async_runtime::spawn(run_frame_reader(
            frame_stream,
            frame_tx.clone(),
            info_tx.clone(),
            shutdown_rx.clone(),
            StreamInfoTracker::new(width, height, pixel_format),
            self.events.clone(),
        ));

        // Spawn WS server task.
        let ws_handle = {
            let state = viva_streamer::ws::AppState { frame_tx, info_tx };
            tauri::async_runtime::spawn(async move {
                if let Err(e) = viva_streamer::ws::run_server_with_listener(
                    listener,
                    "/ws".to_string(),
                    state,
                    shutdown_rx,
                )
                .await
                {
                    warn!("WS server task error: {e}");
                }
            })
        };

        // Receiving GVSP frames does not keep the GVCP control channel alive, but
        // the keepalive that covers it belongs to `GigeRegisterIo` and lives as
        // long as the connected camera — it is not tied to an acquisition, so a
        // stalled consumer (or an idle session with no acquisition at all) cannot
        // let control privilege expire.

        *self.acquisition.lock().await = Some(AcquisitionState {
            shutdown_tx,
            task_handles: vec![frame_reader_handle, ws_handle],
        });

        Ok(StreamerInfo {
            ws_url,
            width,
            height,
        })
    }

    async fn stop_acquisition(&self) -> Result<(), String> {
        self.stop_acquisition_inner().await;
        // Stop acquisition on the camera, if one is still connected.
        match self
            .with_camera(|connected| {
                connected
                    .camera
                    .acquisition_stop()
                    .map_err(|e| format!("Failed to stop acquisition: {e}"))
            })
            .await
        {
            Err(e) if e == "No camera connected" => Ok(()),
            other => other,
        }
    }

    async fn shutdown(&self) {
        self.release_camera().await;
    }

    async fn report(&self, include_xml: bool) -> Result<String, String> {
        use viva_genicam::report::{ReportRequest, ReportTarget, build_report};

        let connected = self
            .with_camera(|connected| {
                let ip = connected
                    .camera
                    .transport()
                    .lock_device()
                    .map(|device| device.remote_addr().ip())
                    .map_err(|e| e.to_string())?;
                Ok((connected.device_id.clone(), ip))
            })
            .await
            .ok();
        let mut notes = vec!["mode:        embedded".to_string()];
        let target = match connected {
            Some((device_id, std::net::IpAddr::V4(ip))) => {
                notes.push(format!("connected:   {device_id}"));
                ReportTarget::Ip(ip)
            }
            _ => ReportTarget::None,
        };
        let request = ReportRequest {
            notes,
            include_xml,
            ..ReportRequest::new(studio_tool(), target)
        };
        Ok(build_report(&request).await)
    }

    async fn force_ip(
        &self,
        mac: [u8; 6],
        ip: Ipv4Addr,
        subnet: Ipv4Addr,
        gateway: Ipv4Addr,
    ) -> Result<(), String> {
        // Out of the interface the camera answered discovery on. Without one
        // the OS picks the interface, usually the default route's, which on a
        // direct camera link is a different network card.
        let name = self.interfaces.read().await.get(&mac).cloned();
        let iface = match name {
            Some(name) => Some(
                viva_genicam::gige::nic::Iface::from_system(&name)
                    .map_err(|e| format!("ForceIP: interface {name}: {e}"))?,
            ),
            None => None,
        };
        viva_genicam::gige::force_ip(mac, ip, subnet, gateway, iface.as_ref())
            .await
            .map_err(|e| format!("ForceIP failed: {e}"))?;
        info!(%ip, %subnet, %gateway, "ForceIP command sent");
        Ok(())
    }

    async fn get_network_config(&self) -> Result<NetworkConfig, String> {
        let (current_ip, persistent_ip, persistent_subnet, persistent_gateway, device_id) = self
            .with_camera(|connected| {
                let mut device_guard = connected
                    .camera
                    .transport()
                    .lock_device()
                    .map_err(|e| format!("Failed to access device: {e}"))?;
                let current_ip = device_guard.remote_addr().ip().to_string();
                let handle = tokio::runtime::Handle::current();
                let (pip, psub, pgw) = handle
                    .block_on(device_guard.read_persistent_ip())
                    .map_err(|e| format!("Failed to read persistent IP: {e}"))?;
                Ok((
                    current_ip,
                    pip.to_string(),
                    psub.to_string(),
                    pgw.to_string(),
                    connected.device_id.clone(),
                ))
            })
            .await?;

        let mac = {
            let discovered = self.discovered.read().await;
            discovered
                .iter()
                .find(|d| d.id == device_id)
                .and_then(|d| d.mac.clone())
                .unwrap_or_default()
        };

        Ok(NetworkConfig {
            current_ip,
            persistent_ip,
            persistent_subnet,
            persistent_gateway,
            mac,
        })
    }

    async fn set_persistent_ip(
        &self,
        ip: Ipv4Addr,
        subnet: Ipv4Addr,
        gateway: Ipv4Addr,
    ) -> Result<(), String> {
        self.with_camera(move |connected| {
            let mut device_guard = connected
                .camera
                .transport()
                .lock_device()
                .map_err(|e| format!("Failed to access device: {e}"))?;
            let handle = tokio::runtime::Handle::current();
            handle
                .block_on(device_guard.write_persistent_ip(ip, subnet, gateway))
                .map_err(|e| format!("Failed to write persistent IP: {e}"))?;
            handle
                .block_on(device_guard.enable_persistent_ip())
                .map_err(|e| format!("Failed to enable persistent IP: {e}"))?;
            Ok(())
        })
        .await?;

        info!(%ip, %subnet, %gateway, "Persistent IP configured and enabled");
        Ok(())
    }
}

// ── Streaming ───────────────────────────────────────────────────────────────

/// Read frames until shutdown, sending the viewer a BMP of each as often as
/// [`FramePacer`] allows: at most [`DISPLAY_FPS`] a second, and once a second
/// while no viewer is connected, so one that connects later has a frame.
/// Once a second it also reports the stream's frame rate and dropped frames,
/// as `viva-service` does (ST-36).
async fn run_frame_reader(
    mut frame_stream: FrameStream,
    frame_tx: watch::Sender<Bytes>,
    info_tx: watch::Sender<viva_streamer::ws::StreamInfo>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut info: StreamInfoTracker,
    events: Arc<Events>,
) {
    let mut pacer = FramePacer::new(Duration::from_secs(1) / DISPLAY_FPS);
    let mut last_status = Instant::now();
    let mut encoder = FrameEncoder::new();
    let mut unsupported = None;
    let mut logged_first = false;

    loop {
        let frame = tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
            result = frame_stream.next_frame() => match result {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    info!("Frame stream ended");
                    break;
                }
                Err(e) => {
                    warn!("Frame stream error: {e}");
                    break;
                }
            },
        };
        if !logged_first {
            info!(
                width = frame.width,
                height = frame.height,
                pixel_format = ?frame.pixel_format,
                "First frame received (embedded)"
            );
            logged_first = true;
        }
        if let Some(changed) =
            info.observe(frame.width, frame.height, &frame.pixel_format.to_string())
        {
            let _ = info_tx.send(changed);
        }
        if last_status.elapsed() >= STATUS_INTERVAL {
            let stats = frame_stream.stats();
            events.send(&AcquisitionStatus {
                active: true,
                fps: Some(stats.avg_fps as f32),
                dropped: stats.drops,
            });
            last_status = Instant::now();
        }
        if !pacer.is_due(frame_tx.receiver_count() > 0, Instant::now()) {
            continue;
        }
        let Some(layout) = camera_pixel_layout(frame.pixel_format) else {
            if unsupported != Some(frame.pixel_format) {
                warn!(pixel_format = ?frame.pixel_format, "Pixel format cannot be drawn; dropping its frames");
                unsupported = Some(frame.pixel_format);
            }
            continue;
        };
        match encoder.encode(layout, frame.width, frame.height, frame.payload.as_ref()) {
            Ok(bmp) => {
                // Stored even with no viewer: the next one to connect is sent it.
                frame_tx.send_replace(bmp);
                pacer.sent(Instant::now());
            }
            Err(e) => warn!("BMP encode error: {e}"),
        }
    }
}

/// The stream description the viewer last got, re-sent when a frame
/// disagrees with it: Studio seeds it from `PixelFormat` before the first
/// frame, and the camera may change format or size mid-stream (ST-17).
struct StreamInfoTracker {
    width: u32,
    height: u32,
    pixel_format: String,
}

impl StreamInfoTracker {
    fn new(width: u32, height: u32, pixel_format: String) -> Self {
        Self {
            width,
            height,
            pixel_format,
        }
    }

    /// The description to send for a frame, if it differs from the last.
    fn observe(
        &mut self,
        width: u32,
        height: u32,
        pixel_format: &str,
    ) -> Option<viva_streamer::ws::StreamInfo> {
        if (width, height) == (self.width, self.height) && pixel_format == self.pixel_format {
            return None;
        }
        self.width = width;
        self.height = height;
        self.pixel_format = pixel_format.to_string();
        Some(camera_stream_info(width, height, pixel_format))
    }
}

/// How a camera's PFNC format is drawn, or `None` if it cannot be.
fn camera_pixel_layout(format: viva_genicam::pfnc::PixelFormat) -> Option<PixelLayout> {
    use BayerPattern::{Bggr, Gbrg, Grbg, Rggb};
    use viva_genicam::pfnc::PixelFormat as P;
    let bayer = |pattern, bits| PixelLayout::Bayer { pattern, bits };
    Some(match format {
        P::Mono8 => PixelLayout::Mono { bits: 8 },
        P::Mono10 => PixelLayout::Mono { bits: 10 },
        P::Mono12 => PixelLayout::Mono { bits: 12 },
        P::Mono14 => PixelLayout::Mono { bits: 14 },
        P::Mono16 => PixelLayout::Mono { bits: 16 },
        P::BayerRG8 => bayer(Rggb, 8),
        P::BayerGR8 => bayer(Grbg, 8),
        P::BayerBG8 => bayer(Bggr, 8),
        P::BayerGB8 => bayer(Gbrg, 8),
        P::BayerRG16 => bayer(Rggb, 16),
        P::BayerGR16 => bayer(Grbg, 16),
        P::BayerBG16 => bayer(Bggr, 16),
        P::BayerGB16 => bayer(Gbrg, 16),
        P::RGB8Packed => PixelLayout::Rgb8,
        P::BGR8Packed => PixelLayout::Bgr8,
        _ => return None,
    })
}

// ── Discovery helpers ───────────────────────────────────────────────────────

async fn discover_gige_devices() -> (Vec<DeviceInfo>, HashMap<[u8; 6], String>) {
    let timeout = Duration::from_secs(1);
    // `discover` rather than `discover_all`: the latter probes loopback, which
    // on Windows can raise WSAECONNRESET and used to abort the whole scan,
    // leaving the UI on "No device" with a real camera on the wire (#57).
    // `discover_all` remains for the fake-camera tests that need loopback.
    match viva_genicam::gige::discover(timeout).await {
        Ok(devices) => discovered_gige(devices),
        Err(e) => {
            warn!("GigE discovery failed: {e}");
            (Vec::new(), HashMap::new())
        }
    }
}

/// The device list for what discovery found, and the interface each camera
/// answered on, which FORCEIP must leave through (ST-37).
///
/// Each device is the announce the service would publish for it, so the id
/// is the MAC-derived one and not the IP (ST-25): the IP changes under DHCP,
/// FORCEIP and a persistent-IP write from this very app, and the bridge has
/// always keyed by MAC. `connect` resolves the IP back.
fn discovered_gige(
    devices: Vec<viva_genicam::gige::DeviceInfo>,
) -> (Vec<DeviceInfo>, HashMap<[u8; 6], String>) {
    let interfaces = devices
        .iter()
        .filter_map(|d| Some((d.mac, d.interface.clone()?)))
        .collect();
    let infos = devices
        .into_iter()
        .map(|d| {
            let announce = DeviceAnnounce::for_gige(
                &d.mac,
                d.ip,
                d.model.as_deref(),
                d.serial.as_deref(),
                d.user_name.as_deref(),
                d.manufacturer.as_deref(),
            );
            DeviceInfo::from_announce(announce, "gige")
        })
        .collect();
    (infos, interfaces)
}

/// Convert a JSON value to a string suitable for the camera `set()` API.
fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// The tool line of a report Studio writes.
pub(crate) fn studio_tool() -> String {
    format!("Viva Studio {}", env!("CARGO_PKG_VERSION"))
}

fn camera_stream_info(
    width: u32,
    height: u32,
    pixel_format: impl Into<String>,
) -> viva_streamer::ws::StreamInfo {
    viva_streamer::ws::StreamInfo {
        width,
        height,
        pixel_format: pixel_format.into(),
        encoding: "BMP",
        frame_type: "info",
    }
}

/// The in-process fake GigE camera, for tests that drive this backend end to
/// end. Every such test binds UDP 3956, so they take [`fake_camera::lock`]
/// first.
#[cfg(test)]
pub(crate) mod fake_camera {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use viva_fake_gige::registers::{CCP, REG_ACQ_STOP};
    use viva_fake_gige::{FakeCamera, GvcpCommand};

    use super::EmbeddedBackend;
    use crate::backend::DeviceBackend;

    static PORT_3956: Mutex<()> = Mutex::new(());

    /// Serialise the tests that bind the GVCP port.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        PORT_3956
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) async fn start() -> FakeCamera {
        loop {
            let built = FakeCamera::builder()
                .bind_ip([127, 0, 0, 1].into())
                .port(3956)
                .width(64)
                .height(48)
                .fps(30)
                .build()
                .await;
            match built {
                Ok(camera) => return camera,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("failed to start the fake camera: {e}"),
            }
        }
    }

    /// Connect to `fake`, start streaming, and check the camera is held:
    /// without that the release assertions would pass vacuously.
    pub(crate) async fn connect_and_acquire(fake: &FakeCamera) -> EmbeddedBackend {
        let backend = EmbeddedBackend::new(Default::default());
        backend.connect("127.0.0.1").await.expect("connect");
        backend
            .start_acquisition(Default::default())
            .await
            .expect("start acquisition");
        assert_ne!(
            fake.peek(CCP, 4).await,
            vec![0; 4],
            "precondition: the backend holds control privilege"
        );
        fake.commands().reset();
        backend
    }

    /// The first image the viewer's WebSocket delivers: a BMP file, since
    /// that is what both backends encode frames into.
    pub(crate) async fn first_frame(ws_url: &str) -> Vec<u8> {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message;

        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .expect("connect to the image WebSocket");
        let wait = async {
            while let Some(message) = socket.next().await {
                if let Ok(Message::Binary(bytes)) = message {
                    return bytes.to_vec();
                }
            }
            panic!("the image WebSocket closed before a frame arrived");
        };
        let frame = tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .expect("no frame within 10 s");
        assert_eq!(&frame[..2], b"BM", "a frame is a BMP file");
        frame
    }

    /// The camera was told to stop acquiring and no longer has a controller.
    pub(crate) async fn assert_released(fake: &FakeCamera) {
        let stops = fake.commands().at(GvcpCommand::WriteReg, REG_ACQ_STOP)
            + fake.commands().at(GvcpCommand::WriteMem, REG_ACQ_STOP);
        assert!(stops >= 1, "AcquisitionStop was never written");
        assert_eq!(
            fake.peek(CCP, 4).await,
            vec![0; 4],
            "control privilege is still held"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{camera_stream_info, fake_camera};
    use crate::backend::DeviceBackend;

    /// Disconnecting must leave the camera free for the next client: not
    /// still streaming into a closed socket, and not still claimed until its
    /// heartbeat runs out.
    #[test]
    fn disconnect_stops_acquisition_and_releases_control() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = fake_camera::connect_and_acquire(&fake).await;

            backend.disconnect().await.expect("disconnect");

            fake_camera::assert_released(&fake).await;
        });
    }

    /// The embedded backend reports what the camera permits now, through the
    /// same `viva-genapi` computation `viva-service` uses (ST-26). Its own copy
    /// hard-coded `is_available` to `true` and offered every `PixelFormat`
    /// entry the XML declares (ST-18). The fake gates all three on registers,
    /// so it can disagree.
    #[test]
    fn feature_state_reports_availability_locks_and_accepted_entries() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let _fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");

            // A monochrome sensor implements only the mono formats.
            let pixel_format = backend.get_feature_state("PixelFormat").await.unwrap();
            let mut entries = pixel_format.enum_available.expect("enum entries");
            entries.sort();
            assert_eq!(entries, ["Mono16", "Mono8"]);

            // `AcquisitionFrameRateEnable` gates `AcquisitionFrameRate`.
            let frame_rate = backend
                .get_feature_state("AcquisitionFrameRate")
                .await
                .unwrap();
            assert!(frame_rate.is_available, "the fake boots it enabled");
            backend
                .set_feature("AcquisitionFrameRateEnable", &serde_json::json!(false))
                .await
                .expect("AcquisitionFrameRateEnable");
            let frame_rate = backend
                .get_feature_state("AcquisitionFrameRate")
                .await
                .unwrap();
            assert!(!frame_rate.is_available);

            // `ExposureAuto` locks `ExposureTime` while it is not `Off`, and
            // the write says so: `ExposureTime` is among what it invalidated.
            let exposure = backend.get_feature_state("ExposureTime").await.unwrap();
            assert_eq!(exposure.access_mode, "RW");
            let invalidated = backend
                .set_feature("ExposureAuto", &serde_json::json!("Continuous"))
                .await
                .expect("ExposureAuto");
            assert!(
                invalidated.iter().any(|n| n == "ExposureTime"),
                "{invalidated:?}"
            );
            let exposure = backend.get_feature_state("ExposureTime").await.unwrap();
            assert_eq!(exposure.access_mode, "RO");

            backend.disconnect().await.expect("disconnect");
        });
    }

    /// Frames reach the viewer: acquisition starts on the camera and BMP
    /// images arrive on the WebSocket the backend hands the UI.
    #[test]
    fn frames_reach_the_image_websocket() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let _fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let streamer = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            assert_eq!((streamer.width, streamer.height), (64, 48));

            fake_camera::first_frame(&streamer.ws_url).await;

            backend.stop_acquisition().await.expect("stop");
            backend.disconnect().await.expect("disconnect");
        });
    }

    /// A viewer that connects after the camera has stopped sending still gets
    /// its last frame. In SingleFrame mode the one frame arrives before the
    /// UI opens the WebSocket, and the reader skipped every frame while no
    /// viewer was connected (Codex review of #173).
    #[test]
    fn a_viewer_that_connects_late_gets_the_last_frame() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let _fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let streamer = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");

            // Frames arrive with nobody watching; then the camera stops.
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            backend
                .exec_command("AcquisitionStop")
                .await
                .expect("AcquisitionStop");
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            fake_camera::first_frame(&streamer.ws_url).await;

            backend.stop_acquisition().await.expect("stop");
            backend.disconnect().await.expect("disconnect");
        });
    }

    /// While streaming, the backend reports the camera's frame rate and the
    /// frames lost on the way, from the library's stream statistics, once a
    /// second. Embedded mode reported neither, and the header showed only
    /// "Acquiring" (ST-36).
    #[test]
    fn streaming_reports_the_frame_rate_and_the_dropped_frames() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let _fake = fake_camera::start().await;
            let (events, recorder) = crate::state::events::Recorder::events();
            let backend = super::EmbeddedBackend::new(events);
            backend.connect("127.0.0.1").await.expect("connect");
            backend
                .start_acquisition(Default::default())
                .await
                .expect("start");

            let reported = async {
                loop {
                    let streaming = recorder.of("acquisition-status").into_iter().find(|s| {
                        s["active"] == true && s["fps"].as_f64().is_some_and(|fps| fps > 0.0)
                    });
                    if let Some(status) = streaming {
                        return status;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let status = tokio::time::timeout(std::time::Duration::from_secs(5), reported)
                .await
                .expect("no acquisition-status with a frame rate within 5 s");
            assert_eq!(status["dropped"], 0, "{status}");

            backend.stop_acquisition().await.expect("stop");
            backend.disconnect().await.expect("disconnect");
        });
    }

    /// The device panel's IP configuration (ST-37): the backend reads the
    /// camera's current and persistent address, and writes a persistent one
    /// the camera keeps. Tauri refused these commands until #175 granted
    /// them, so nothing had run this path.
    #[test]
    fn network_config_reads_and_writes_the_persistent_address() {
        use viva_fake_gige::registers::{
            PERSISTENT_DEFAULT_GATEWAY, PERSISTENT_IP_ADDRESS, PERSISTENT_SUBNET_MASK,
        };
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");

            let config = backend.get_network_config().await.expect("read");
            assert_eq!(config.current_ip, "127.0.0.1");
            assert_eq!(config.persistent_ip, "0.0.0.0", "the fake boots with none");

            backend
                .set_persistent_ip(
                    "192.168.1.100".parse().unwrap(),
                    "255.255.255.0".parse().unwrap(),
                    "192.168.1.1".parse().unwrap(),
                )
                .await
                .expect("write");
            assert_eq!(
                fake.peek(PERSISTENT_IP_ADDRESS, 4).await,
                [192, 168, 1, 100]
            );
            assert_eq!(
                fake.peek(PERSISTENT_SUBNET_MASK, 4).await,
                [255, 255, 255, 0]
            );
            assert_eq!(
                fake.peek(PERSISTENT_DEFAULT_GATEWAY, 4).await,
                [192, 168, 1, 1]
            );
            let config = backend.get_network_config().await.expect("read back");
            assert_eq!(config.persistent_ip, "192.168.1.100");

            backend.disconnect().await.expect("disconnect");
        });
    }

    /// FORCEIP leaves through the interface a camera answered discovery on,
    /// so the backend keeps it per camera; a record discovery did not make
    /// has none (ST-37).
    #[test]
    fn discovery_keeps_the_interface_each_camera_answered_on() {
        let mac = [0x00, 0x0C, 0xDF, 0x01, 0x02, 0x03];
        let found = viva_genicam::gige::DeviceInfo {
            mac,
            interface: Some("en7".to_string()),
            ..viva_genicam::gige::DeviceInfo::from_ip("169.254.1.20".parse().unwrap())
        };
        let typed = viva_genicam::gige::DeviceInfo::from_ip("10.0.0.2".parse().unwrap());

        let (devices, interfaces) = super::discovered_gige(vec![found, typed]);

        assert_eq!(devices.len(), 2);
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[&mac], "en7");
    }

    /// The camera's `GevSCPSPacketSize`, low 16 bits.
    async fn packet_size_on(fake: &viva_fake_gige::FakeCamera) -> u32 {
        use viva_fake_gige::registers::{SCP_PACKET_SIZE, STREAM_CHANNEL_BASE};
        let bytes = fake.peek(STREAM_CHANNEL_BASE + SCP_PACKET_SIZE, 4).await;
        u32::from_be_bytes(bytes.try_into().unwrap()) & 0xFFFF
    }

    /// A packet size the user measured for their link is what the camera is
    /// told, rather than one negotiated from the NIC (ST-22, #112).
    #[test]
    fn a_measured_packet_size_is_used_instead_of_negotiating() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");

            let options = super::StartOptions {
                packet_size: Some(1000),
            };
            backend.start_acquisition(options).await.expect("start");
            assert_eq!(packet_size_on(&fake).await, 1000);
            backend.stop_acquisition().await.expect("stop");

            // Negative control: left to negotiate, loopback's MTU gives more.
            backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            assert_ne!(packet_size_on(&fake).await, 1000);
            backend.disconnect().await.expect("disconnect");
        });
    }

    /// "Generate report" writes the report `viva-camctl report` would, on the
    /// connected camera (ST-41).
    #[test]
    fn a_report_describes_the_connected_camera() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let _fake = fake_camera::start().await;
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let report = backend.report(true).await.expect("report");
            for expected in [
                "tool:        Viva Studio",
                "mode:        embedded",
                "connected:   127.0.0.1",
                "selected: 127.0.0.1",
                "<RegisterDescription",
            ] {
                assert!(report.contains(expected), "missing {expected:?}:\n{report}");
            }
            backend.disconnect().await.expect("disconnect");

            // With nothing connected, the report still covers the host.
            let report = backend.report(false).await.expect("report");
            assert!(report.contains("No camera selected"), "{report}");
        });
    }

    /// The viewer is told when a frame's format or size differs from what it
    /// was last told, and only then (ST-17).
    #[test]
    fn stream_info_is_resent_when_a_frame_disagrees() {
        let mut info = super::StreamInfoTracker::new(64, 48, "Mono8".to_string());
        assert!(info.observe(64, 48, "Mono8").is_none());
        let changed = info.observe(64, 48, "RGB8Packed").expect("format changed");
        assert_eq!(changed.pixel_format, "RGB8Packed");
        assert!(info.observe(64, 48, "RGB8Packed").is_none());
        let resized = info.observe(32, 24, "RGB8Packed").expect("size changed");
        assert_eq!((resized.width, resized.height), (32, 24));
    }

    #[test]
    fn stream_info_preserves_camera_pixel_format() {
        let info = camera_stream_info(2048, 1536, "BayerRG8");

        assert_eq!(info.width, 2048);
        assert_eq!(info.height, 1536);
        assert_eq!(info.pixel_format, "BayerRG8");
        assert_eq!(info.encoding, "BMP");
    }
}
