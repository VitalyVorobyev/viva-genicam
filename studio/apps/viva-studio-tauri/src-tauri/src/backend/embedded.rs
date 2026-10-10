//! Embedded device backend: direct GigE Vision and USB3 Vision camera access.
//!
//! This backend uses `viva-genicam` to communicate directly with cameras without
//! requiring an external service process. It is the default mode when no Zenoh
//! configuration is detected.
//!
//! ## Thread safety
//!
//! `Camera<GigeRegisterIo>` is `!Sync` because the underlying `NodeMap` uses
//! `RefCell` for caching. All camera operations are dispatched to a dedicated
//! blocking thread via `spawn_blocking` and a `std::sync::Mutex` protects
//! concurrent access.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::{Mutex as AsyncMutex, RwLock, watch};
use tracing::{debug, info, warn};
use viva_genicam::{Camera, FrameStream, GigeRegisterIo};
use viva_zenoh_api::{DeviceAnnounce, FeatureState};

use crate::state::device_state::{DeviceInfo, StreamerInfo};
use crate::state::events::Events;

use super::{BackendMode, ConnectResult, DeviceBackend, NetworkConfig};

// ── Internal types ──────────────────────────────────────────────────────────

/// A connected GigE Vision camera and its associated state.
///
/// Wrapped in a `std::sync::Mutex` because `Camera<GigeRegisterIo>` is `!Sync`
/// (the `NodeMap` uses `RefCell` for caching). All access goes through
/// `spawn_blocking`.
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
    /// Kept for diagnostics and status reporting.
    #[allow(dead_code)]
    ws_url: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
}

// ── EmbeddedBackend ─────────────────────────────────────────────────────────

/// How long each step of releasing a camera waits for its acknowledgement.
/// Short, because the camera may already be gone and app exit waits on it.
const RELEASE_ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// Backend that communicates directly with cameras via GigE Vision / USB3 Vision.
pub struct EmbeddedBackend {
    /// Connected camera behind a std::sync::Mutex for spawn_blocking access.
    camera: Arc<std::sync::Mutex<Option<ConnectedCamera>>>,
    /// Cache of discovered devices, updated periodically.
    discovered: RwLock<Vec<DeviceInfo>>,
    /// Active acquisition state, if streaming.
    acquisition: AsyncMutex<Option<AcquisitionState>>,
    events: Arc<Events>,
}

impl EmbeddedBackend {
    /// Create a new embedded backend with no connected camera.
    pub fn new(events: Arc<Events>) -> Self {
        Self {
            camera: Arc::new(std::sync::Mutex::new(None)),
            discovered: RwLock::new(Vec::new()),
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
                let devices = discover_gige_devices().await;
                *backend.discovered.write().await = devices.clone();
                for d in &devices {
                    backend.events.emit("device-discovered", d);
                }
            }
        });
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
    /// its keepalive (`GigeRegisterIo`'s `Drop`), which needs the runtime this
    /// runs on.
    async fn release_camera(&self) {
        self.stop_acquisition_inner().await;
        let connected = match self.camera.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(mut connected) = connected else {
            return;
        };
        tokio::task::block_in_place(move || {
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
        });
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

        let result = ConnectResult {
            xml,
            device_name: name,
            model,
        };

        {
            let mut guard = self
                .camera
                .lock()
                .map_err(|_| "Camera mutex poisoned".to_string())?;
            *guard = Some(ConnectedCamera {
                camera,
                device_id: device_id.to_string(),
            });
        }

        Ok(result)
    }

    async fn disconnect(&self) -> Result<(), String> {
        self.release_camera().await;
        Ok(())
    }

    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let name = name.to_string();
        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        // Camera::get() calls RegisterIo::read() which uses block_on() internally.
        // block_in_place converts the current async thread to a blocking thread,
        // allowing nested block_on to work without panic.
        tokio::task::block_in_place(|| connected.feature_state(&name))
    }

    async fn set_feature(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let value_str = json_value_to_string(value);
        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        tokio::task::block_in_place(|| connected.camera.set(name, &value_str))
            .map_err(|e| format!("Failed to write feature '{name}': {e}"))?;
        Ok(connected.camera.nodemap().invalidated_by(name))
    }

    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String> {
        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        tokio::task::block_in_place(|| connected.camera.set(name, ""))
            .map_err(|e| format!("Failed to execute command '{name}': {e}"))?;
        Ok(connected.camera.nodemap().invalidated_by(name))
    }

    async fn bulk_feature_state(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, FeatureState>, String> {
        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        let result = tokio::task::block_in_place(|| {
            let mut result = HashMap::with_capacity(names.len());
            for name in names {
                match connected.feature_state(name) {
                    Ok(state) => {
                        result.insert(name.clone(), state);
                    }
                    Err(e) => {
                        tracing::warn!(name, error = %e, "Failed to read feature in bulk_feature_state");
                    }
                }
            }
            result
        });
        Ok(result)
    }

    async fn start_acquisition(&self) -> Result<StreamerInfo, String> {
        // Stop any previous acquisition.
        self.stop_acquisition_inner().await;

        // Build the stream and start acquisition on a blocking thread
        // (GigeRegisterIo uses block_on() internally, must not run in async context).
        let (width, height, pixel_format, frame_stream) = {
            let mut guard = self
                .camera
                .lock()
                .map_err(|_| "Camera mutex poisoned".to_string())?;
            let connected = guard
                .as_mut()
                .ok_or_else(|| "No camera connected".to_string())?;

            // `block_in_place` puts no `Send` bound on its closure, so the
            // borrow can be handed over directly — the raw-pointer reborrow
            // that used to stand here bought nothing and cost the aliasing
            // guarantee.
            let cam = &mut connected.camera;

            tokio::task::block_in_place(move || {
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

                // Get device handle for stream building.
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
                // Windows selected to reach that camera.
                let iface = viva_genicam::gige::nic::Iface::from_remote_ipv4(camera_ipv4)
                    .map_err(|e| format!("Failed to detect network interface: {e}"))?;

                let stream = handle
                    .block_on(
                        viva_genicam::StreamBuilder::new(&mut device_guard)
                            .iface(iface)
                            .rcvbuf_bytes(64 << 20) // 64 MiB to absorb bursty GVSP traffic
                            // No packet-size UI yet; negotiate from NIC MTU + path probe.
                            .auto_packet_size()
                            .build(),
                    )
                    .map_err(|e| format!("Failed to build stream: {e}"))?;

                drop(device_guard);

                let frame_stream = FrameStream::new(stream, None);

                // Start acquisition on the camera.
                cam.acquisition_start()
                    .map_err(|e| format!("Failed to start acquisition: {e}"))?;

                Ok((width, height, pixel_format, frame_stream))
            })?
        };

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

        // Spawn frame reader task.
        let frame_reader_handle = {
            let frame_tx = frame_tx.clone();
            let info_tx = info_tx.clone();
            let mut shutdown_rx = shutdown_rx.clone();
            let mut frame_stream = frame_stream;
            tauri::async_runtime::spawn(async move {
                let mut encoder_gray = viva_streamer::bmp::BmpEncoder::new(width, height);
                let mut encoder_rgb = viva_streamer::bmp::BmpEncoder::new_rgb24(width, height);
                let mut stream_width = width;
                let mut stream_height = height;
                let mut stream_pixel_format = pixel_format;
                let mut logged_first = false;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                        result = frame_stream.next_frame() => {
                            match result {
                                Ok(Some(frame)) => {
                                    if !logged_first {
                                        info!(
                                            width = frame.width,
                                            height = frame.height,
                                            pixel_format = ?frame.pixel_format,
                                            "First frame received (embedded)"
                                        );
                                        logged_first = true;
                                    }

                                    if frame.width != stream_width || frame.height != stream_height {
                                        encoder_gray = viva_streamer::bmp::BmpEncoder::new(
                                            frame.width,
                                            frame.height,
                                        );
                                        encoder_rgb = viva_streamer::bmp::BmpEncoder::new_rgb24(
                                            frame.width,
                                            frame.height,
                                        );
                                    }

                                    let frame_pixel_format = frame.pixel_format.to_string();
                                    if frame.width != stream_width
                                        || frame.height != stream_height
                                        || frame_pixel_format != stream_pixel_format
                                    {
                                        stream_width = frame.width;
                                        stream_height = frame.height;
                                        stream_pixel_format = frame_pixel_format;
                                        let _ = info_tx.send(camera_stream_info(
                                            stream_width,
                                            stream_height,
                                            stream_pixel_format.clone(),
                                        ));
                                    }

                                    let bmp = encode_camera_frame(
                                        &frame,
                                        &encoder_gray,
                                        &encoder_rgb,
                                    );
                                    match bmp {
                                        Ok(Some(data)) => {
                                            let _ = frame_tx.send(data);
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            warn!("BMP encode error: {e}");
                                        }
                                    }
                                }
                                Ok(None) => {
                                    info!("Frame stream ended");
                                    break;
                                }
                                Err(e) => {
                                    warn!("Frame stream error: {e}");
                                    break;
                                }
                            }
                        }
                    }
                }
            })
        };

        // Spawn WS server task.
        let ws_handle = {
            let state = viva_streamer::ws::AppState { frame_tx, info_tx };
            let shutdown_rx = shutdown_rx.clone();
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
            ws_url: ws_url.clone(),
            width,
            height,
        });

        Ok(StreamerInfo {
            ws_url,
            width,
            height,
        })
    }

    async fn stop_acquisition(&self) -> Result<(), String> {
        self.stop_acquisition_inner().await;

        // Stop acquisition on the camera.
        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        if let Some(connected) = guard.as_mut() {
            tokio::task::block_in_place(|| connected.camera.acquisition_stop())
                .map_err(|e| format!("Failed to stop acquisition: {e}"))?;
        }

        Ok(())
    }

    async fn shutdown(&self) {
        self.release_camera().await;
    }

    async fn force_ip(
        &self,
        mac: [u8; 6],
        ip: Ipv4Addr,
        subnet: Ipv4Addr,
        gateway: Ipv4Addr,
    ) -> Result<(), String> {
        viva_genicam::gige::force_ip(mac, ip, subnet, gateway, None)
            .await
            .map_err(|e| format!("ForceIP failed: {e}"))?;
        info!(%ip, %subnet, %gateway, "ForceIP command sent");
        Ok(())
    }

    async fn get_network_config(&self) -> Result<NetworkConfig, String> {
        let (current_ip, persistent_ip, persistent_subnet, persistent_gateway, device_id) = {
            let guard = self
                .camera
                .lock()
                .map_err(|_| "Camera mutex poisoned".to_string())?;
            let connected = guard
                .as_ref()
                .ok_or_else(|| "No camera connected".to_string())?;
            let device_id = connected.device_id.clone();

            tokio::task::block_in_place(|| {
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

                Ok::<_, String>((
                    current_ip,
                    pip.to_string(),
                    psub.to_string(),
                    pgw.to_string(),
                    device_id,
                ))
            })?
        };

        // Read discovery cache after dropping the camera mutex.
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
        let guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard
            .as_ref()
            .ok_or_else(|| "No camera connected".to_string())?;

        tokio::task::block_in_place(|| {
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

            Ok::<_, String>(())
        })?;

        info!(%ip, %subnet, %gateway, "Persistent IP configured and enabled");
        Ok(())
    }
}

// ── Discovery helpers ───────────────────────────────────────────────────────

async fn discover_gige_devices() -> Vec<DeviceInfo> {
    let timeout = Duration::from_secs(1);
    // `discover` rather than `discover_all`: the latter probes loopback, which
    // on Windows can raise WSAECONNRESET and used to abort the whole scan,
    // leaving the UI on "No device" with a real camera on the wire (#57).
    // `discover_all` remains for the fake-camera tests that need loopback.
    match viva_genicam::gige::discover(timeout).await {
        // The same announce the service would publish for this camera, so the
        // id is the MAC-derived one and not the IP (ST-25): the IP changes under
        // DHCP, FORCEIP and a persistent-IP write from this very app, and the
        // bridge has always keyed by MAC. `connect` resolves the IP back.
        Ok(devices) => devices
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
            .collect(),
        Err(e) => {
            warn!("GigE discovery failed: {e}");
            Vec::new()
        }
    }
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

// ── Frame encoding ──────────────────────────────────────────────────────────

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

/// Convert a `viva_genicam::Frame` to BMP bytes for WebSocket delivery.
fn encode_camera_frame(
    frame: &viva_genicam::Frame,
    encoder_gray: &viva_streamer::bmp::BmpEncoder,
    encoder_rgb: &viva_streamer::bmp::BmpEncoder,
) -> Result<Option<Bytes>, String> {
    use viva_genicam::pfnc::PixelFormat;

    let pixel_data = frame.payload.as_ref();

    match frame.pixel_format {
        PixelFormat::Mono8 => encoder_gray
            .encode_gray8(pixel_data)
            .map(Some)
            .map_err(|e| e.to_string()),

        PixelFormat::Mono16 => {
            let gray = viva_streamer::bmp::mono_u16le_to_gray8(pixel_data, 16);
            encoder_gray
                .encode_gray8(&gray)
                .map(Some)
                .map_err(|e| e.to_string())
        }

        PixelFormat::RGB8Packed => encoder_rgb
            .encode_rgb24(pixel_data)
            .map(Some)
            .map_err(|e| e.to_string()),

        PixelFormat::BGR8Packed => {
            let rgb: Vec<u8> = pixel_data
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| [c[2], c[1], c[0]])
                .collect();
            encoder_rgb
                .encode_rgb24(&rgb)
                .map(Some)
                .map_err(|e| e.to_string())
        }

        PixelFormat::BayerRG8
        | PixelFormat::BayerGR8
        | PixelFormat::BayerBG8
        | PixelFormat::BayerGB8 => {
            let pattern = camera_bayer_pattern(frame.pixel_format);
            let rgb =
                viva_streamer::bmp::debayer_nn(pixel_data, frame.width, frame.height, pattern);
            encoder_rgb
                .encode_rgb24(&rgb)
                .map(Some)
                .map_err(|e| e.to_string())
        }

        _ => {
            warn!(
                pixel_format = ?frame.pixel_format,
                "Unsupported pixel format in embedded streaming"
            );
            Ok(None)
        }
    }
}

fn camera_bayer_pattern(pf: viva_genicam::pfnc::PixelFormat) -> viva_streamer::bmp::BayerPattern {
    use viva_genicam::pfnc::PixelFormat;
    use viva_streamer::bmp::BayerPattern;

    match pf {
        PixelFormat::BayerRG8 => BayerPattern::Rggb,
        PixelFormat::BayerGR8 => BayerPattern::Grbg,
        PixelFormat::BayerBG8 => BayerPattern::Bggr,
        PixelFormat::BayerGB8 => BayerPattern::Gbrg,
        _ => BayerPattern::Rggb,
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
            .start_acquisition()
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
            let streamer = backend.start_acquisition().await.expect("start");
            assert_eq!((streamer.width, streamer.height), (64, 48));

            fake_camera::first_frame(&streamer.ws_url).await;

            backend.stop_acquisition().await.expect("stop");
            backend.disconnect().await.expect("disconnect");
        });
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
