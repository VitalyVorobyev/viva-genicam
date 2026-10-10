//! Embedded device backend: direct GigE Vision and USB3 Vision camera access.
//!
//! This backend uses `viva-genicam` to communicate directly with cameras without
//! requiring an external service process. It is the default mode when no Zenoh
//! configuration is detected.
//!
//! ## Thread safety
//!
//! A camera, GigE Vision or USB3 Vision ([`super::camera::Device`]), is `!Sync`:
//! its `NodeMap` caches in `RefCell`s, and its register I/O blocks the calling
//! thread. So the camera
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
use tracing::{info, warn};
use viva_genicam::Camera;
use viva_genicam::u3v::discovery::U3vDeviceInfo;
use viva_streamer::bmp::BayerPattern;
use viva_streamer::frame::{FrameEncoder, FramePacer, PixelLayout};
use viva_zenoh_api::{AcquisitionStatus, DeviceAnnounce, FeatureState};

use crate::state::device_state::{DeviceInfo, StreamerInfo};
use crate::state::events::Events;

use super::camera::{Device, Frames};
use super::{BackendMode, ConnectResult, DeviceBackend, NetworkConfig, StartOptions};

// ── Internal types ──────────────────────────────────────────────────────────

/// A connected camera, GigE Vision or USB3 Vision, and its associated state.
///
/// `Send` is derived, not asserted: every field is `Send` already. The
/// `unsafe impl Send` this type used to carry was redundant, and a redundant
/// unsafe impl is worse than none — it would have gone on silently covering
/// for a genuinely non-`Send` field added later.
struct ConnectedCamera {
    camera: Box<dyn Device>,
    /// The id discovery reported, to find the camera's MAC again.
    device_id: String,
    /// What the diagnostic report says about a camera it cannot open itself:
    /// a USB3 Vision device admits one owner, and this app is it (DX-06).
    /// `None` for GigE Vision, which the report opens read-only.
    report: Option<viva_genicam::report::OpenCamera>,
}

/// State for an active image acquisition session.
struct AcquisitionState {
    shutdown_tx: watch::Sender<bool>,
    /// Reads frames; shuts its stream down on the way out.
    reader: tauri::async_runtime::JoinHandle<()>,
    /// Serves the viewer's WebSocket.
    server: tauri::async_runtime::JoinHandle<()>,
}

// ── EmbeddedBackend ─────────────────────────────────────────────────────────

/// How long each step of releasing a camera waits for its acknowledgement.
/// Short, because the camera may already be gone and app exit waits on it.
const RELEASE_ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// The most frames a second the viewer is sent. A camera can deliver far
/// more, and encoding each as a BMP costs CPU the user cannot see.
const DISPLAY_FPS: u32 = 30;

/// How long stopping waits for the WebSocket server to finish on its own.
const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// How often a stream reports its frame rate and dropped frames.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Backend that communicates directly with cameras via GigE Vision / USB3 Vision.
pub struct EmbeddedBackend {
    /// The connected camera; see the module docs.
    camera: Arc<AsyncMutex<Option<ConnectedCamera>>>,
    /// Cache of discovered devices, updated periodically.
    discovered: RwLock<Vec<DeviceInfo>>,
    /// The USB3 Vision cameras the last scan found, by device id.
    usb: RwLock<HashMap<String, U3vDeviceInfo>>,
    /// The id each camera present at a USB position was given, keyed by the
    /// position and its vendor and product: a camera keeps its serial-based id
    /// when a later scan cannot read the serial. Positions a scan no longer
    /// finds are forgotten.
    u3v_ids: RwLock<HashMap<UsbPosition, String>>,
    /// The host address each camera's discovery reply arrived at, by MAC.
    /// Kept across scans: one scan can miss a camera the list still shows.
    local_ips: RwLock<HashMap<[u8; 6], Ipv4Addr>>,
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
            local_ips: RwLock::new(HashMap::new()),
            usb: RwLock::new(HashMap::new()),
            u3v_ids: RwLock::new(HashMap::new()),
            acquisition: AsyncMutex::new(None),
            events,
        }
    }

    /// Start a background task that periodically discovers GigE Vision and
    /// USB3 Vision cameras.
    pub fn start_discovery_task(self: &Arc<Self>, interval: Duration) {
        let backend = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let gige = discover_gige_devices().await;
                let usb = discover_u3v_devices().await;
                backend.record_scan(gige, usb).await;
            }
        });
    }

    /// Take in what one discovery scan found, and announce it.
    async fn record_scan(
        &self,
        found: Vec<viva_genicam::gige::DeviceInfo>,
        usb: Vec<U3vDeviceInfo>,
    ) {
        let (mut devices, local_ips) = discovered_gige(found);
        let mut by_id = HashMap::new();
        let mut known = self.u3v_ids.write().await;
        // A camera that is gone takes its id with it, so a device that later
        // takes its place does not inherit it (Codex review of #180).
        let present: Vec<UsbPosition> = usb.iter().map(UsbPosition::of).collect();
        known.retain(|position, _| present.contains(position));
        for info in usb {
            let position = UsbPosition::of(&info);
            // A serial that cannot be read now — the camera may be busy, open
            // in this app — keeps the id it had (Codex review of #179).
            let id = match (&info.serial, known.get(&position)) {
                (None, Some(id)) => id.clone(),
                _ => u3v_id(&info),
            };
            known.insert(position, id.clone());
            let device = u3v_device_info(&info, id.clone());
            by_id.insert(id, info);
            devices.push(device);
        }
        drop(known);
        *self.usb.write().await = by_id;
        *self.discovered.write().await = devices.clone();
        // Updated, never cleared: a camera one scan misses keeps its address
        // (Codex review of #177).
        self.local_ips.write().await.extend(local_ips);
        for d in &devices {
            self.events.send(d);
        }
    }

    /// The interface FORCEIP to `mac` leaves through: the one the camera
    /// answered discovery on, with the address that discovery used — a NIC
    /// can carry several (Codex review of #177) — or `None` to let the OS
    /// pick.
    async fn forceip_interface(
        &self,
        mac: &[u8; 6],
    ) -> Result<Option<viva_genicam::gige::nic::Iface>, String> {
        let local_ip = self.local_ips.read().await.get(mac).copied();
        local_ip
            .map(|ip| {
                viva_genicam::gige::nic::Iface::from_ipv4(ip)
                    .map_err(|e| format!("ForceIP: the interface with {ip}: {e}"))
            })
            .transpose()
    }

    /// Open a USB3 Vision camera discovery found (ST-14). Opening it reads its
    /// descriptors and XML over USB, so on the blocking pool.
    async fn connect_u3v(
        &self,
        device_id: &str,
        info: U3vDeviceInfo,
    ) -> Result<ConnectResult, String> {
        info!(
            device_id,
            "Connecting to USB3 Vision camera (embedded mode)"
        );
        let model = info
            .model
            .clone()
            .unwrap_or_else(|| "USB3 Vision camera".to_string());
        let identity = u3v_identity(&info);
        let (camera, xml) =
            tauri::async_runtime::spawn_blocking(move || viva_genicam::connect_u3v_with_xml(&info))
                .await
                .map_err(|e| format!("Camera task failed: {e}"))?
                .map_err(|e| format!("Failed to open USB3 Vision camera {device_id}: {e}"))?;
        let report = viva_genicam::report::OpenCamera {
            transport: "USB3 Vision".to_string(),
            identity,
            xml: xml.clone(),
        };
        self.attach(Box::new(camera), device_id, Some(report)).await;
        Ok(ConnectResult {
            xml,
            device_name: model.clone(),
            model,
        })
    }

    /// Hold `camera` as the connected one.
    async fn attach(
        &self,
        camera: Box<dyn Device>,
        device_id: &str,
        report: Option<viva_genicam::report::OpenCamera>,
    ) {
        *self.camera.lock().await = Some(ConnectedCamera {
            camera,
            device_id: device_id.to_string(),
            report,
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
            // The reader is waited for, however long: it shuts its stream
            // down on the way out, and a USB read it is inside ends within
            // the transfer timeout. Abandoning it would leave it on the
            // endpoint for the next stream (Codex review of #180).
            let _ = state.reader.await;
            // The WebSocket server holds nothing of the camera's.
            let mut server = state.server;
            if tokio::time::timeout(TASK_STOP_TIMEOUT, &mut server)
                .await
                .is_err()
            {
                server.abort();
                let _ = server.await;
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
            connected.camera.release(RELEASE_ACK_TIMEOUT);
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

        let usb = self.usb.read().await.get(device_id).cloned();
        if let Some(info) = usb {
            return self.connect_u3v(device_id, info).await;
        }

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

        self.attach(Box::new(camera), device_id, None).await;

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
        self.with_camera(move |connected| connected.camera.feature_state(&name))
            .await
    }

    async fn set_feature(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let value = json_value_to_string(value);
        let name = name.to_string();
        self.with_camera(move |connected| connected.camera.set(&name, &value))
            .await
    }

    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String> {
        let name = name.to_string();
        self.with_camera(move |connected| connected.camera.set(&name, ""))
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
                match connected.camera.feature_state(&name) {
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

        let (width, height, pixel_format, frames) = self
            .with_camera(move |connected| {
                let (width, height, pixel_format) = connected.camera.image_format();
                let frames = connected.camera.start(&options)?;
                Ok((width, height, pixel_format, frames))
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
            frames,
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
            reader: frame_reader_handle,
            server: ws_handle,
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
            .with_camera(|connected| connected.camera.acquisition_stop())
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
                let ip = gige_of(connected).ok().and_then(|camera| {
                    camera
                        .transport()
                        .lock_device()
                        .map(|device| device.remote_addr().ip())
                        .ok()
                });
                Ok((connected.device_id.clone(), ip, connected.report.clone()))
            })
            .await
            .ok();
        let mut notes = vec!["mode:        embedded".to_string()];
        let target = match connected {
            // A USB3 Vision camera this app holds: the report cannot open it
            // again, so it is described from what was read at connect (DX-06).
            Some((device_id, _, Some(open))) => {
                notes.push(format!("connected:   {device_id}"));
                ReportTarget::Open(open)
            }
            Some((device_id, Some(std::net::IpAddr::V4(ip)), None)) => {
                notes.push(format!("connected:   {device_id}"));
                ReportTarget::Ip(ip)
            }
            Some((device_id, _, None)) => {
                notes.push(format!("connected:   {device_id}"));
                ReportTarget::None
            }
            None => ReportTarget::None,
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
        let iface = self.forceip_interface(&mac).await?;
        viva_genicam::gige::force_ip(mac, ip, subnet, gateway, iface.as_ref())
            .await
            .map_err(|e| format!("ForceIP failed: {e}"))?;
        info!(%ip, %subnet, %gateway, "ForceIP command sent");
        Ok(())
    }

    async fn get_network_config(&self) -> Result<NetworkConfig, String> {
        let (current_ip, persistent_ip, persistent_subnet, persistent_gateway, device_id) = self
            .with_camera(|connected| {
                let device_id = connected.device_id.clone();
                let mut device_guard = gige_of(connected)?
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
                    device_id,
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
            let mut device_guard = gige_of(connected)?
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

/// The connected camera as GigE Vision, for what only GigE Vision has.
fn gige_of(
    connected: &mut ConnectedCamera,
) -> Result<&mut Camera<viva_genicam::GigeRegisterIo>, String> {
    connected
        .camera
        .gige()
        .ok_or_else(|| "Only a GigE Vision camera has an IP configuration".to_string())
}

// ── Streaming ───────────────────────────────────────────────────────────────

/// Read frames until shutdown, sending the viewer a BMP of each as often as
/// [`FramePacer`] allows: at most [`DISPLAY_FPS`] a second, and once a second
/// while no viewer is connected, so one that connects later has a frame.
/// Once a second it also reports the stream's frame rate and dropped frames,
/// as `viva-service` does (ST-36).
async fn run_frame_reader(
    mut frame_stream: Frames,
    frame_tx: watch::Sender<Bytes>,
    info_tx: watch::Sender<viva_streamer::ws::StreamInfo>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut info: StreamInfoTracker,
    events: Arc<Events>,
) {
    let mut pacer = FramePacer::new(Duration::from_secs(1) / DISPLAY_FPS);
    let mut last_status = Instant::now();
    let mut frames_since_status = 0u32;
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
        frames_since_status += 1;
        let elapsed = last_status.elapsed();
        if elapsed >= STATUS_INTERVAL {
            events.send(&AcquisitionStatus {
                active: true,
                fps: Some(frames_since_status as f32 / elapsed.as_secs_f32()),
                dropped: frame_stream.dropped(),
            });
            last_status = Instant::now();
            frames_since_status = 0;
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
    // Wait for the stream to let go of the camera before this task ends: the
    // backend awaits this task before it starts another stream or releases
    // the camera.
    frame_stream.shutdown().await;
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

async fn discover_gige_devices() -> Vec<viva_genicam::gige::DeviceInfo> {
    let timeout = Duration::from_secs(1);
    // `discover` rather than `discover_all`: the latter probes loopback, which
    // on Windows can raise WSAECONNRESET and used to abort the whole scan,
    // leaving the UI on "No device" with a real camera on the wire (#57).
    // `discover_all` remains for the fake-camera tests that need loopback.
    viva_genicam::gige::discover(timeout)
        .await
        .unwrap_or_else(|e| {
            warn!("GigE discovery failed: {e}");
            Vec::new()
        })
}

/// USB3 Vision cameras on this host. USB enumeration blocks, so on the
/// blocking pool; a host without USB access finds none.
async fn discover_u3v_devices() -> Vec<U3vDeviceInfo> {
    let found = tauri::async_runtime::spawn_blocking(viva_genicam::u3v::discovery::discover).await;
    match found {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            tracing::debug!("USB3 Vision discovery failed: {e}");
            Vec::new()
        }
        Err(e) => {
            warn!("USB3 Vision discovery task failed: {e}");
            Vec::new()
        }
    }
}

/// A USB3 Vision camera's identity, for the diagnostic report.
fn u3v_identity(info: &U3vDeviceInfo) -> Vec<(String, String)> {
    let opt = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
    vec![
        (
            "usb".to_string(),
            format!(
                "{:04x}:{:04x} on bus {} address {}",
                info.vendor_id, info.product_id, info.bus, info.address
            ),
        ),
        ("manufacturer".to_string(), opt(&info.manufacturer)),
        ("model".to_string(), opt(&info.model)),
        ("serial".to_string(), opt(&info.serial)),
    ]
}

/// Where a USB3 Vision camera is plugged in, and what it is: the key under
/// which its id is remembered between scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UsbPosition {
    bus: u8,
    address: u8,
    vendor_id: u16,
    product_id: u16,
}

impl UsbPosition {
    fn of(info: &U3vDeviceInfo) -> Self {
        Self {
            bus: info.bus,
            address: info.address,
            vendor_id: info.vendor_id,
            product_id: info.product_id,
        }
    }
}

/// A USB3 Vision camera's id. USB has no MAC or IP, so it is the vendor and
/// product, told apart by the serial, or by the bus position when the camera
/// reports none.
fn u3v_id(info: &U3vDeviceInfo) -> String {
    let instance = match &info.serial {
        Some(serial) => serial.clone(),
        None => format!("{}.{}", info.bus, info.address),
    };
    format!(
        "u3v-{:04x}{:04x}-{instance}",
        info.vendor_id, info.product_id
    )
}

/// A USB3 Vision camera as the device list shows it, under `id`.
fn u3v_device_info(info: &U3vDeviceInfo, id: String) -> DeviceInfo {
    let model = info
        .model
        .clone()
        .unwrap_or_else(|| "USB3 Vision camera".to_string());
    let mut announce = DeviceAnnounce::new(
        id,
        model.clone(),
        model,
        info.serial.clone().unwrap_or_default(),
    );
    announce.manufacturer = info.manufacturer.clone();
    DeviceInfo::from_announce(announce, "usb3")
}

/// The device list for what discovery found, and the host address each
/// camera's reply arrived at, which FORCEIP must leave from (ST-37).
///
/// Each device is the announce the service would publish for it, so the id
/// is the MAC-derived one and not the IP (ST-25): the IP changes under DHCP,
/// FORCEIP and a persistent-IP write from this very app, and the bridge has
/// always keyed by MAC. `connect` resolves the IP back.
fn discovered_gige(
    devices: Vec<viva_genicam::gige::DeviceInfo>,
) -> (Vec<DeviceInfo>, HashMap<[u8; 6], Ipv4Addr>) {
    let local_ips = devices
        .iter()
        .filter_map(|d| Some((d.mac, d.local_ip?)))
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
    (infos, local_ips)
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

    /// FORCEIP leaves from the host address a camera answered discovery at,
    /// so the backend keeps it per camera; a record discovery did not make
    /// has none (ST-37).
    #[test]
    fn discovery_keeps_the_interface_each_camera_answered_on() {
        let mac = [0x00, 0x0C, 0xDF, 0x01, 0x02, 0x03];
        let found = viva_genicam::gige::DeviceInfo {
            mac,
            local_ip: Some("169.254.1.1".parse().unwrap()),
            ..viva_genicam::gige::DeviceInfo::from_ip("169.254.1.20".parse().unwrap())
        };
        let typed = viva_genicam::gige::DeviceInfo::from_ip("10.0.0.2".parse().unwrap());

        let (devices, local_ips) = super::discovered_gige(vec![found, typed]);

        assert_eq!(devices.len(), 2);
        assert_eq!(local_ips.len(), 1);
        assert_eq!(
            local_ips[&mac],
            "169.254.1.1".parse::<std::net::Ipv4Addr>().unwrap()
        );
    }

    /// One discovery scan can miss a camera that answered the last one, while
    /// the device list still shows it; FORCEIP to it must still leave through
    /// its interface, not the one the OS picks (Codex review of #177).
    #[test]
    fn a_missed_scan_keeps_the_interface_a_camera_answered_on() {
        tauri::async_runtime::block_on(async {
            let mac = [0x00, 0x0C, 0xDF, 0x01, 0x02, 0x03];
            let found = viva_genicam::gige::DeviceInfo {
                mac,
                local_ip: Some(std::net::Ipv4Addr::LOCALHOST),
                ..viva_genicam::gige::DeviceInfo::from_ip("169.254.1.20".parse().unwrap())
            };
            let backend = super::EmbeddedBackend::new(Default::default());

            backend.record_scan(vec![found], vec![]).await;
            backend.record_scan(vec![], vec![]).await;

            let iface = backend.forceip_interface(&mac).await.expect("resolve");
            let iface = iface.expect("the interface was forgotten after one missed scan");
            // The address discovery used, not the interface's first one.
            assert_eq!(iface.ipv4(), Some(std::net::Ipv4Addr::LOCALHOST));
        });
    }

    /// A USB3 Vision camera works in embedded mode: its features read and
    /// write, and its frames reach the viewer. Embedded mode held only a GigE
    /// Vision camera, so a USB3 Vision one needed `viva-service-u3v` (ST-14).
    #[test]
    fn a_usb3_vision_camera_reads_writes_and_streams() {
        tauri::async_runtime::block_on(async {
            let device = viva_fake_u3v::FakeU3vCamera::new(64, 48)
                .open_device()
                .expect("fake U3V camera");
            let (camera, xml) = viva_genicam::open_u3v_device(device).expect("open");
            let backend = super::EmbeddedBackend::new(Default::default());
            let open = viva_genicam::report::OpenCamera {
                transport: "USB3 Vision".to_string(),
                identity: vec![("model".to_string(), "FakeU3V".to_string())],
                xml,
            };
            backend
                .attach(Box::new(camera), "u3v-fake", Some(open))
                .await;

            let width = backend.get_feature_state("Width").await.expect("Width");
            assert_eq!(width.value, serde_json::json!(64));
            backend
                .set_feature("Width", &serde_json::json!(32))
                .await
                .expect("write Width");
            let width = backend.get_feature_state("Width").await.expect("Width");
            assert_eq!(width.value, serde_json::json!(32));

            let streamer = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            assert_eq!((streamer.width, streamer.height), (32, 48));
            fake_camera::first_frame(&streamer.ws_url).await;
            backend.stop_acquisition().await.expect("stop");

            // Stopping waits for the USB reader, so a stream started right
            // away has the endpoint to itself (Codex review of #179).
            let streamer = backend
                .start_acquisition(Default::default())
                .await
                .expect("restart");
            fake_camera::first_frame(&streamer.ws_url).await;
            backend.stop_acquisition().await.expect("stop again");

            // An IP configuration is GigE Vision's alone.
            let refused = backend.get_network_config().await.unwrap_err();
            assert!(refused.contains("GigE Vision"), "{refused}");

            // The report describes the camera this app holds (DX-06).
            let report = backend.report(false).await.expect("report");
            assert!(report.contains("transport: USB3 Vision"), "{report}");
            assert!(report.contains("model: FakeU3V"), "{report}");

            backend.disconnect().await.expect("disconnect");
        });
    }

    /// Two cameras of one model are told apart by serial, or by bus position
    /// when the camera reports none.
    #[test]
    fn a_usb3_vision_camera_is_named_by_serial_or_bus_position() {
        use viva_genicam::u3v::descriptor::U3vInterfaceInfo;
        use viva_genicam::u3v::discovery::U3vDeviceInfo;
        let info = |serial: Option<&str>| U3vDeviceInfo {
            bus: 2,
            address: 7,
            vendor_id: 0x2676,
            product_id: 0xba02,
            serial: serial.map(str::to_string),
            manufacturer: Some("Vendor".to_string()),
            model: Some("Model".to_string()),
            interface_info: U3vInterfaceInfo {
                control_iface: 0,
                control_ep_in: 0x81,
                control_ep_out: 0x01,
                stream_iface: Some(1),
                stream_ep_in: Some(0x82),
                event_iface: None,
                event_ep_in: None,
            },
        };
        assert_eq!(super::u3v_id(&info(Some("A123"))), "u3v-2676ba02-A123");
        assert_eq!(super::u3v_id(&info(None)), "u3v-2676ba02-2.7");
        let listed = super::u3v_device_info(&info(Some("A123")), "u3v-x".to_string());
        assert_eq!(
            (listed.id.as_str(), listed.transport.as_str()),
            ("u3v-x", "usb3")
        );
    }

    /// A camera keeps its id when a later scan cannot read its serial — it
    /// may be busy, open in this very app — so the list does not grow a
    /// second entry for it and its first id still connects (Codex review of
    /// #179).
    #[test]
    fn a_usb3_vision_camera_keeps_its_id_when_its_serial_goes_unread() {
        tauri::async_runtime::block_on(async {
            let backend = super::EmbeddedBackend::new(Default::default());
            backend
                .record_scan(vec![], vec![u3v_info(Some("A123"))])
                .await;
            backend.record_scan(vec![], vec![u3v_info(None)]).await;

            let ids: Vec<String> = backend.discover().await.into_iter().map(|d| d.id).collect();
            assert_eq!(ids, ["u3v-2676ba02-A123"]);
            assert!(backend.usb.read().await.contains_key("u3v-2676ba02-A123"));
        });
    }

    /// A remembered id belongs to one camera: a different device at the same
    /// USB position, or the same position after the camera was unplugged,
    /// does not inherit it (Codex review of #180).
    #[test]
    fn a_usb3_vision_id_is_not_inherited() {
        tauri::async_runtime::block_on(async {
            let backend = super::EmbeddedBackend::new(Default::default());
            let ids = || async {
                backend
                    .discover()
                    .await
                    .into_iter()
                    .map(|d| d.id)
                    .collect::<Vec<_>>()
            };

            // Another model at the camera's position, its serial unread.
            backend
                .record_scan(vec![], vec![u3v_info(Some("A123"))])
                .await;
            let mut other = u3v_info(None);
            other.product_id = 0x0001;
            backend.record_scan(vec![], vec![other]).await;
            assert_eq!(ids().await, ["u3v-26760001-2.7"]);

            // The camera unplugged, then a camera of the same model there,
            // its serial unread: nothing says it is the same one.
            backend
                .record_scan(vec![], vec![u3v_info(Some("A123"))])
                .await;
            backend.record_scan(vec![], vec![]).await;
            backend.record_scan(vec![], vec![u3v_info(None)]).await;
            assert_eq!(ids().await, ["u3v-2676ba02-2.7"]);
        });
    }

    fn u3v_info(serial: Option<&str>) -> viva_genicam::u3v::discovery::U3vDeviceInfo {
        viva_genicam::u3v::discovery::U3vDeviceInfo {
            bus: 2,
            address: 7,
            vendor_id: 0x2676,
            product_id: 0xba02,
            serial: serial.map(str::to_string),
            manufacturer: Some("Vendor".to_string()),
            model: Some("Model".to_string()),
            interface_info: viva_genicam::u3v::descriptor::U3vInterfaceInfo {
                control_iface: 0,
                control_ep_in: 0x81,
                control_ep_out: 0x01,
                stream_iface: Some(1),
                stream_ep_in: Some(0x82),
                event_iface: None,
                event_ep_in: None,
            },
        }
    }

    /// Stopping waits for a USB read that stalls mid-frame. It gave up after
    /// two seconds and abandoned the reader inside a five-second bulk read,
    /// so a restart could put a second reader on the endpoint (Codex review
    /// of #180).
    #[test]
    fn stopping_waits_for_a_stalled_usb_read() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        use std::time::Duration;
        use viva_genicam::u3v::U3vError;
        use viva_genicam::u3v::usb::UsbTransfer;

        /// The fake camera, with stream reads that can be made to hang.
        struct Stalling {
            inner: viva_fake_u3v::FakeU3vTransport,
            stall: AtomicBool,
            reading: AtomicBool,
        }
        impl UsbTransfer for Stalling {
            fn bulk_write(&self, ep: u8, data: &[u8], t: Duration) -> Result<usize, U3vError> {
                self.inner.bulk_write(ep, data, t)
            }
            fn bulk_read(&self, ep: u8, buf: &mut [u8], t: Duration) -> Result<usize, U3vError> {
                if ep == 0x82 && self.stall.load(SeqCst) {
                    self.reading.store(true, SeqCst);
                    std::thread::sleep(Duration::from_secs(3));
                    self.reading.store(false, SeqCst);
                }
                self.inner.bulk_read(ep, buf, t)
            }
        }

        tauri::async_runtime::block_on(async {
            let transport = Arc::new(Stalling {
                inner: viva_fake_u3v::FakeU3vTransport::new(64, 48, 0x0108_0001),
                stall: AtomicBool::new(false),
                reading: AtomicBool::new(false),
            });
            let device = viva_genicam::u3v::device::U3vDevice::open(
                transport.clone(),
                0x81,
                0x01,
                Some(0x82),
                None,
            )
            .expect("open");
            let (camera, _xml) = viva_genicam::open_u3v_device(device).expect("camera");
            let backend = super::EmbeddedBackend::new(Default::default());
            backend.attach(Box::new(camera), "u3v-stall", None).await;
            let streamer = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            fake_camera::first_frame(&streamer.ws_url).await;

            transport.stall.store(true, SeqCst);
            let stalled = async {
                while !transport.reading.load(SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(2), stalled)
                .await
                .expect("the reader never reached a stream read");

            backend.stop_acquisition().await.expect("stop");
            assert!(
                !transport.reading.load(SeqCst),
                "stop returned while the USB reader was still inside a read"
            );
            backend.disconnect().await.expect("disconnect");
        });
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
