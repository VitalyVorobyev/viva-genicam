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

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use tauri::Emitter;
use tokio::sync::{Mutex as AsyncMutex, RwLock, watch};
use tracing::{info, warn};
use viva_genicam::genapi::{AccessMode, Node, NodeMap, RegisterIo};
use viva_genicam::{Camera, FrameStream, GigeRegisterIo};
use viva_zenoh_api::{DeviceAnnounce, FeatureState, NumericRange};

use crate::state::device_state::{DeviceInfo, NodeValueEntry, StreamerInfo};

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
    /// Nodes a command executes through ([`NodeMap::command_targets`]),
    /// computed once at connect because every feature snapshot consults it.
    command_targets: HashSet<String>,
    /// Kept for logging and disconnect matching.
    #[allow(dead_code)]
    device_id: String,
    #[allow(dead_code)]
    xml: String,
}

impl ConnectedCamera {
    /// Snapshot one feature of this camera.
    fn feature_state(&self, name: &str) -> Result<FeatureState, String> {
        build_feature_state(
            self.camera.nodemap(),
            self.camera.transport(),
            &self.command_targets,
            name,
        )
    }
}

/// State for an active image acquisition session.
struct AcquisitionState {
    shutdown_tx: watch::Sender<bool>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Kept for diagnostics and status reporting.
    #[allow(dead_code)]
    ws_url: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
}

// ── EmbeddedBackend ─────────────────────────────────────────────────────────

/// Backend that communicates directly with cameras via GigE Vision / USB3 Vision.
pub struct EmbeddedBackend {
    /// Connected camera behind a std::sync::Mutex for spawn_blocking access.
    camera: Arc<std::sync::Mutex<Option<ConnectedCamera>>>,
    /// Cache of discovered devices, updated periodically.
    discovered: RwLock<Vec<DeviceInfo>>,
    /// Active acquisition state, if streaming.
    acquisition: AsyncMutex<Option<AcquisitionState>>,
}

impl EmbeddedBackend {
    /// Create a new embedded backend with no connected camera.
    pub fn new() -> Self {
        Self {
            camera: Arc::new(std::sync::Mutex::new(None)),
            discovered: RwLock::new(Vec::new()),
            acquisition: AsyncMutex::new(None),
        }
    }

    /// Start a background task that periodically discovers GigE cameras.
    pub fn start_discovery_task(self: &Arc<Self>, app: tauri::AppHandle, interval: Duration) {
        let backend = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let devices = discover_gige_devices().await;
                *backend.discovered.write().await = devices.clone();
                for d in &devices {
                    let _ = app.emit("device-discovered", d);
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
            }
        }
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
        self.disconnect(device_id).await.ok();

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
            xml: xml.clone(),
            device_name: name,
            model,
        };

        {
            let mut guard = self
                .camera
                .lock()
                .map_err(|_| "Camera mutex poisoned".to_string())?;
            let command_targets = camera.nodemap().command_targets();
            *guard = Some(ConnectedCamera {
                camera,
                command_targets,
                device_id: device_id.to_string(),
                xml,
            });
        }

        Ok(result)
    }

    async fn disconnect(&self, _device_id: &str) -> Result<(), String> {
        self.stop_acquisition_inner().await;
        {
            let mut guard = self
                .camera
                .lock()
                .map_err(|_| "Camera mutex poisoned".to_string())?;
            *guard = None;
        }
        info!("Camera disconnected (embedded mode)");
        Ok(())
    }

    async fn get_feature(&self, name: &str) -> Result<NodeValueEntry, String> {
        // Project the rich state into the legacy entry so existing callers
        // (Tauri commands, cache updaters) keep working during migration.
        let state = self.get_feature_state(name).await?;
        Ok(feature_state_to_entry(&state))
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

    async fn set_feature(&self, name: &str, value: &serde_json::Value) -> Result<(), String> {
        let value_str = json_value_to_string(value);
        let name = name.to_string();

        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        tokio::task::block_in_place(|| connected.camera.set(&name, &value_str))
            .map_err(|e| format!("Failed to write feature '{name}': {e}"))
    }

    async fn exec_command(&self, name: &str) -> Result<(), String> {
        let name = name.to_string();

        let mut guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        let connected = guard.as_mut().ok_or("No camera connected".to_string())?;
        tokio::task::block_in_place(|| connected.camera.set(&name, ""))
            .map_err(|e| format!("Failed to execute command '{name}': {e}"))
    }

    async fn bulk_read(&self, names: &[String]) -> Result<HashMap<String, NodeValueEntry>, String> {
        let states = self.bulk_feature_state(names).await?;
        Ok(states
            .into_iter()
            .map(|(k, s)| (k, feature_state_to_entry(&s)))
            .collect())
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
            tokio::spawn(async move {
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
            tokio::spawn(async move {
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

    async fn get_xml(&self) -> Result<String, String> {
        let guard = self
            .camera
            .lock()
            .map_err(|_| "Camera mutex poisoned".to_string())?;
        guard
            .as_ref()
            .map(|c| c.xml.clone())
            .ok_or_else(|| "No camera connected".to_string())
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

// ── Feature state construction ──────────────────────────────────────────────

/// Build a [`FeatureState`] snapshot for `name` by reading the live value
/// through the correct typed accessor on the NodeMap and collecting metadata
/// (access mode, numeric range, available enum entries, unit).
///
/// This replaces the old `string_to_json_value` heuristic that tried to infer
/// the value's type by sniffing the string from `Camera::get()`. Sniffing led
/// to Enum entries literally named `"0"` being silently coerced to integers
/// and to Float registers with bit-pattern-encoded f64 values being parsed as
/// i64 strings. Typed dispatch based on `Node::kind_name` is the correct
/// primitive.
///
/// NOTE: the underlying typed readers (`get_integer`, `get_float`, etc.) may
/// themselves be buggy in `viva-genapi` — see
/// [ADR-0010](../../../../../docs/adrs/adr0010-feature-state-contract.md) —
/// but at least we now dispatch to the right one.
///
/// `command_targets` is [`NodeMap::command_targets`]: the nodes a command
/// writes through, which are reported but never read.
fn build_feature_state(
    nodemap: &NodeMap,
    transport: &dyn RegisterIo,
    command_targets: &HashSet<String>,
    name: &str,
) -> Result<FeatureState, String> {
    let node = nodemap
        .node(name)
        .ok_or_else(|| format!("Node '{name}' not found"))?;

    let kind = node.kind_name().to_string();

    // Read a value only from a node that has one to give; mirrors
    // `viva-service`. Categories and Commands have none. A write-only node —
    // declared `WO`, or delegating to a `WO` register — refuses the read, and
    // a command's backing register holds a trigger, not a value; reading one
    // is at best meaningless (ST-21, #112). Both used to be read, and the
    // failure dropped the feature from a bulk read with a WARN (SVC-08, #135).
    // They are reported with a null value instead, so the UI still offers the
    // write or the execute.
    let write_only = nodemap.is_write_only(name);
    let has_value = !write_only && !command_targets.contains(name);
    let access_mode = match node.access_mode() {
        // Declared `RW` but delegating to a `WO` register: say what it is.
        Some(AccessMode::RW) if write_only => "WO".to_string(),
        _ => access_mode_string(node),
    };

    let value = match node {
        _ if !has_value => serde_json::Value::Null,
        Node::Integer(_) => nodemap
            .get_integer(name, transport)
            .map(|v| serde_json::Value::Number(v.into()))
            .map_err(|e| format!("Failed to read integer '{name}': {e}"))?,
        Node::Float(_) => nodemap
            .get_float(name, transport)
            .map(f64_to_json)
            .map_err(|e| format!("Failed to read float '{name}': {e}"))?,
        Node::Enum(_) => nodemap
            .get_enum(name, transport)
            .map(serde_json::Value::String)
            .map_err(|e| format!("Failed to read enum '{name}': {e}"))?,
        Node::Boolean(_) => nodemap
            .get_bool(name, transport)
            .map(serde_json::Value::Bool)
            .map_err(|e| format!("Failed to read bool '{name}': {e}"))?,
        Node::String(_) => nodemap
            .get_string(name, transport)
            .map(serde_json::Value::String)
            .map_err(|e| format!("Failed to read string '{name}': {e}"))?,
        // Report the size, not the bytes: a `<Register>` can be very large and
        // this state is polled by the UI. Mirrors `viva-service`'s handling.
        Node::Register(reg) => serde_json::json!({
            "kind": "register",
            "length": reg.declared_len(),
        }),
        Node::SwissKnife(sk) => match sk.output {
            viva_genicam::genapi::SkOutput::Float => nodemap
                .get_float(name, transport)
                .map(f64_to_json)
                .map_err(|e| format!("Failed to eval SwissKnife '{name}': {e}"))?,
            viva_genicam::genapi::SkOutput::Integer => nodemap
                .get_integer(name, transport)
                .map(|v| serde_json::Value::Number(v.into()))
                .map_err(|e| format!("Failed to eval SwissKnife '{name}': {e}"))?,
        },
        Node::Converter(_) => nodemap
            .get_converter(name, transport)
            .map(f64_to_json)
            .map_err(|e| format!("Failed to eval Converter '{name}': {e}"))?,
        Node::IntConverter(_) => nodemap
            .get_int_converter(name, transport)
            .map(|v| serde_json::Value::Number(v.into()))
            .map_err(|e| format!("Failed to eval IntConverter '{name}': {e}"))?,
        Node::Command(_) | Node::Category(_) => serde_json::Value::Null,
        // `Node` is `#[non_exhaustive]`; name the kind rather than emitting a
        // bare null, so an unhandled type stays legible in the UI. Mirrors
        // `viva-service`.
        other => serde_json::json!({ "kind": other.kind_name(), "unsupported": true }),
    };

    let (numeric, unit) = match node {
        Node::Integer(n) => {
            // When the XML defers bounds to runtime registers (`<pMin>` /
            // `<pMax>`), `n.min` / `n.max` are `i64::MIN` / `i64::MAX`
            // sentinels. Resolve the referenced nodes' current values so the
            // UI can render a real range. A failed pMin/pMax read falls back
            // to the static bound — the UI suppresses sentinel bleed-through.
            let resolved_min = n
                .p_min
                .as_deref()
                .and_then(|pm| nodemap.get_integer(pm, transport).ok())
                .unwrap_or(n.min);
            let resolved_max = n
                .p_max
                .as_deref()
                .and_then(|pm| nodemap.get_integer(pm, transport).ok())
                .unwrap_or(n.max);
            (
                Some(NumericRange {
                    min: resolved_min as f64,
                    max: resolved_max as f64,
                    inc: n.inc.map(|i| i as f64),
                }),
                n.unit.clone(),
            )
        }
        Node::Float(n) => (
            Some(NumericRange {
                min: n.min,
                max: n.max,
                inc: None,
            }),
            n.unit.clone(),
        ),
        _ => (None, None),
    };

    let enum_available = if matches!(node, Node::Enum(_)) {
        // `NodeMap::enum_entries` returns the full set from the enumeration
        // table. `NodeMap::available_enum_entries` now
        // exists and applies the `pIsAvailable` gating this comment used to
        // describe as future work — switching to it is backlog ST-18.
        nodemap.enum_entries(name).ok()
    } else {
        None
    };

    Ok(FeatureState {
        value,
        access_mode,
        kind,
        is_implemented: true,
        is_available: true,
        numeric,
        enum_available,
        unit,
    })
}

/// Map `Node::access_mode()` to the GenICam string spelling, treating `None`
/// (e.g. Category nodes) as `"NA"`.
fn access_mode_string(node: &Node) -> String {
    match node.access_mode() {
        Some(AccessMode::RO) => "RO".to_string(),
        Some(AccessMode::RW) => "RW".to_string(),
        Some(AccessMode::WO) => "WO".to_string(),
        None => "NA".to_string(),
    }
}

/// Convert an `f64` to `serde_json::Value::Number`, falling back to a string
/// when the value is not JSON-representable (NaN, Inf).
fn f64_to_json(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v)
        .map(serde_json::Value::Number)
        .unwrap_or_else(|| serde_json::Value::String(v.to_string()))
}

/// Project a rich [`FeatureState`] into the legacy [`NodeValueEntry`] shape.
fn feature_state_to_entry(state: &FeatureState) -> NodeValueEntry {
    let update = state.to_node_value_update();
    NodeValueEntry {
        value: update.value,
        access_mode: update.access_mode,
        min: update.min,
        max: update.max,
        inc: update.inc,
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};

    use super::{build_feature_state, camera_stream_info};

    #[test]
    fn stream_info_preserves_camera_pixel_format() {
        let info = camera_stream_info(2048, 1536, "BayerRG8");

        assert_eq!(info.width, 2048);
        assert_eq!(info.height, 1536);
        assert_eq!(info.pixel_format, "BayerRG8");
        assert_eq!(info.encoding, "BMP");
    }

    /// A write-only integer, an integer delegating to a write-only register
    /// (`ActionDeviceKey`, as in #112's log), a command reaching an `RW`
    /// register through `<pValue>`, and one ordinary feature.
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>4096</Max>
            </Integer>
            <Integer Name="ActionDeviceKey">
                <pValue>ActionDeviceKeyReg</pValue>
            </Integer>
            <IntReg Name="ActionDeviceKeyReg">
                <Address>0x400</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="SoftwarePulse">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
                <Min>0</Min>
                <Max>1</Max>
            </Integer>
            <Command Name="AcquisitionStart">
                <pValue>AcquisitionStartReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="AcquisitionStartReg">
                <Address>0x300</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    /// Serves `Width` and records every read address.
    #[derive(Default)]
    struct RecordingIo {
        reads: RefCell<Vec<u64>>,
    }

    impl RegisterIo for RecordingIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            self.reads.borrow_mut().push(addr);
            let mut bytes = vec![0; len];
            if addr == 0x100 && len == 4 {
                bytes.copy_from_slice(&640u32.to_be_bytes());
            }
            Ok(bytes)
        }

        fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
            Ok(())
        }
    }

    /// SVC-08 and ST-21: a write-only node and a command's register are
    /// reported, with a null value, instead of being read — the read used to
    /// fail and `bulk_feature_state` dropped the feature with a WARN.
    #[test]
    fn unreadable_nodes_are_reported_without_being_read() {
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let targets = nodemap.command_targets();
        let io = RecordingIo::default();
        let state = |name: &str| {
            build_feature_state(&nodemap, &io, &targets, name)
                .unwrap_or_else(|e| panic!("{name}: {e}"))
        };

        assert_eq!(state("Width").value, serde_json::json!(640));

        let pulse = state("SoftwarePulse");
        assert_eq!(pulse.access_mode, "WO");
        assert!(pulse.value.is_null());

        let key = state("ActionDeviceKey");
        assert_eq!(key.access_mode, "WO");
        assert!(key.value.is_null());

        let command = state("AcquisitionStart");
        assert_eq!(command.kind, "Command");
        assert!(command.value.is_null());

        let register = state("AcquisitionStartReg");
        assert_eq!(register.access_mode, "RW");
        assert!(register.value.is_null());

        let reads = io.reads.borrow();
        assert!(!reads.contains(&0x200), "read the WO register: {reads:x?}");
        assert!(!reads.contains(&0x400), "read the WO register: {reads:x?}");
        assert!(
            !reads.contains(&0x300),
            "read the command register: {reads:x?}"
        );
    }
}
