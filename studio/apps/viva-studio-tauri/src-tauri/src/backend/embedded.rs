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
use viva_genicam::u3v::U3vDeviceInfo;
use viva_genicam::{Camera, ControlState, Reopen};
use viva_streamer::frame::{FrameEncoder, FramePacer};
use viva_zenoh_api::{
    AcquisitionStatus, CameraEvent, ConfigurationLoaded, ConfigurationSaved, DeviceAnnounce,
    EventMonitorStatus, FeatureState, StreamCounters,
};

use crate::commands::xml_model::{ParseXmlResponse, parse_camera_model};
use crate::state::SharedModelState;
use crate::state::connection::Connection;
use crate::state::device_state::{
    ConnectionAccess, ConnectionState, DeviceInfo, DisconnectReason, StreamerInfo,
};
use crate::state::events::{DeviceReconnected, Events, FeaturesInvalidated};

use super::camera::{CameraEvents, Device, Frames};
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
    gige_info: Option<viva_genicam::gige::DeviceInfo>,
    read_only: bool,
    access: ConnectionAccess,
}

impl ConnectedCamera {
    fn writes_allowed(&self) -> bool {
        !self.read_only
            && self
                .camera
                .watch_control()
                .is_none_or(|control| *control.borrow() == ControlState::Held)
    }
}

/// State for an active image acquisition session.
struct AcquisitionState {
    shutdown_tx: watch::Sender<bool>,
    /// Reads frames; shuts its stream down on the way out.
    reader: tauri::async_runtime::JoinHandle<()>,
    /// Serves the viewer's WebSocket.
    server: tauri::async_runtime::JoinHandle<()>,
}

/// Event monitoring that is on (backlog ST-51).
struct EventMonitor {
    shutdown_tx: watch::Sender<bool>,
    /// Forwards events; shuts its channel down on the way out and returns
    /// what it could not undo.
    reader: tauri::async_runtime::JoinHandle<Vec<String>>,
    /// What starting changed, restored once the reader is gone.
    saved: viva_genicam::EventConfiguration,
}

/// One owner for connection transitions. Advancing the watch value cancels
/// asynchronous opens immediately; the transition lock serializes cleanup and
/// publication, including an outstanding blocking USB open.
struct Lifecycle {
    generation: watch::Sender<u64>,
    transition: AsyncMutex<()>,
    shutting_down: std::sync::atomic::AtomicBool,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            generation: watch::channel(0).0,
            transition: AsyncMutex::new(()),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Lifecycle {
    fn begin(&self) -> u64 {
        let mut ticket = 0;
        self.generation.send_modify(|n| {
            *n = n.wrapping_add(1);
            ticket = *n;
        });
        ticket
    }
    fn advance(&self, expected: u64) -> Option<u64> {
        let mut ticket = None;
        self.generation.send_if_modified(|current| {
            if *current != expected {
                return false;
            }
            *current = current.wrapping_add(1);
            ticket = Some(*current);
            true
        });
        ticket
    }

    fn ticket(&self) -> u64 {
        *self.generation.borrow()
    }
    fn current(&self, ticket: u64) -> bool {
        self.ticket() == ticket
    }
}

fn cap_read_only(state: &mut FeatureState) {
    match state.access_mode.as_str() {
        "RW" => state.access_mode = "RO".into(),
        "WO" => state.access_mode = "NA".into(),
        _ => {}
    }
}

fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let bytes: Vec<_> = text
        .split(':')
        .map(|b| u8::from_str_radix(b, 16))
        .collect::<Result<_, _>>()
        .ok()?;
    bytes.try_into().ok()
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

/// How often a stream reports its frame rate and counters.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Backend that communicates directly with cameras via GigE Vision / USB3 Vision.
#[derive(Clone)]
pub struct EmbeddedBackend {
    /// The connected camera; see the module docs.
    camera: Arc<AsyncMutex<Option<ConnectedCamera>>>,
    /// Cache of discovered devices, updated periodically.
    discovered: Arc<RwLock<Vec<DeviceInfo>>>,
    /// The USB3 Vision cameras the last scan found, by device id.
    usb: Arc<RwLock<HashMap<String, U3vDeviceInfo>>>,
    /// The id each camera present at a USB position was given, keyed by the
    /// position and its vendor and product: a camera keeps its serial-based id
    /// when a later scan cannot read the serial. Positions a scan no longer
    /// finds are forgotten.
    u3v_ids: Arc<RwLock<HashMap<UsbPosition, String>>>,
    /// The host address each camera's discovery reply arrived at, by MAC.
    /// Kept across scans: one scan can miss a camera the list still shows.
    local_ips: Arc<RwLock<HashMap<[u8; 6], Ipv4Addr>>>,
    /// Active acquisition state, if streaming.
    acquisition: Arc<AsyncMutex<Option<AcquisitionState>>>,
    /// Event monitoring, if on. Never locked while waiting for the camera.
    event_monitor: Arc<AsyncMutex<Option<EventMonitor>>>,
    events: Arc<Events>,
    lifecycle: Arc<Lifecycle>,
    connection: Arc<Connection>,
    model: SharedModelState,
    reopen_patience: Duration,
    #[cfg(test)]
    reopen_interface: Option<String>,
}

impl EmbeddedBackend {
    /// Create a new embedded backend with no connected camera.
    #[cfg(test)]
    pub fn new(events: Arc<Events>) -> Self {
        Self::with_state(
            events.clone(),
            Arc::new(Connection::new(events)),
            Default::default(),
        )
    }

    pub fn with_state(
        events: Arc<Events>,
        connection: Arc<Connection>,
        model: SharedModelState,
    ) -> Self {
        Self {
            camera: Arc::new(AsyncMutex::new(None)),
            discovered: Default::default(),
            local_ips: Default::default(),
            usb: Default::default(),
            u3v_ids: Default::default(),
            acquisition: Default::default(),
            event_monitor: Default::default(),
            lifecycle: Arc::new(Lifecycle::default()),
            events,
            connection,
            model,
            reopen_patience: Duration::from_secs(30),
            #[cfg(test)]
            reopen_interface: None,
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
        let (mut devices, local_ips) = discovered_gige(found.clone());
        let _transition = self.lifecycle.transition.lock().await;
        if let Some(connected) = self.camera.lock().await.as_mut()
            && let Some(previous) = &connected.gige_info
            && let Some(fresh) = found
                .iter()
                .find(|d| d.mac == previous.mac && d.ip == previous.ip)
        {
            // Never retarget a live socket when discovery finds a changed IP.
            connected.gige_info = Some(fresh.clone());
        }
        let mut by_id = HashMap::new();
        let mut known = self.u3v_ids.write().await;
        // A camera that is gone takes its id with it, so a device that later
        // takes its place does not inherit it (Codex review of #180).
        let present: Vec<UsbPosition> = usb.iter().map(UsbPosition::of).collect();
        known.retain(|position, _| present.contains(position));
        for info in usb {
            let position = UsbPosition::of(&info);
            // A scan that finds no serial keeps the id the camera had (Codex
            // review of #179). Discovery reads the OS's cached descriptors,
            // so a busy camera no longer hides its serial; this stays for a
            // host whose cache lacks it.
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
        let usb_lost = self
            .camera
            .lock()
            .await
            .as_ref()
            .filter(|c| c.report.is_some() && !by_id.contains_key(&c.device_id))
            .map(|_| self.lifecycle.ticket());
        *self.usb.write().await = by_id;
        *self.discovered.write().await = devices.clone();
        // Updated, never cleared: a camera one scan misses keeps its address
        // (Codex review of #177).
        self.local_ips.write().await.extend(local_ips);
        for d in &devices {
            self.events.send(d);
        }
        if let Some(ticket) = usb_lost {
            let backend = self.clone();
            tauri::async_runtime::spawn(async move {
                backend
                    .lose_camera(
                        ticket,
                        "USB3 Vision camera disappeared from enumeration".into(),
                    )
                    .await;
            });
        }
    }

    /// The interface FORCEIP to `mac` leaves through: the one the camera
    /// answered discovery on, with the address that discovery used — a NIC
    /// can carry several (Codex review of #177) — or `None` to let the OS
    /// pick.
    async fn forceip_interface(
        &self,
        mac: &[u8; 6],
    ) -> Result<Option<viva_genicam::gige::Iface>, String> {
        let local_ip = self.local_ips.read().await.get(mac).copied();
        local_ip
            .map(|ip| {
                viva_genicam::gige::Iface::from_ipv4(ip)
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
        let mut identity = u3v_identity(&info);
        let (camera, xml, bootstrap) = tauri::async_runtime::spawn_blocking(move || {
            let (camera, xml) = viva_genicam::connect_u3v_with_xml(&info)?;
            // The bootstrap registers, decoded and raw, for the report:
            // USB3 Vision has met one camera so far (backlog TC-27).
            let bootstrap = camera
                .transport()
                .lock_device()
                .map(|mut device| device.describe())
                .unwrap_or_default();
            Ok::<_, viva_genicam::GenicamError>((camera, xml, bootstrap))
        })
        .await
        .map_err(|e| format!("Camera task failed: {e}"))?
        .map_err(|e| format!("Failed to open USB3 Vision camera {device_id}: {e}"))?;
        identity.extend(bootstrap);
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
            gige_info: None,
            read_only: false,
            access: ConnectionAccess::Control,
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

    /// Stop event monitoring: end the reader, which shuts its channel down,
    /// then — with `restore` — put back the settings starting changed. The
    /// reader is joined before the camera is asked for, and no lock it needs
    /// is held meanwhile. Sends and returns the inactive status, carrying
    /// everything that could not be undone; `None` when nothing was on.
    ///
    /// Without `restore` the camera is gone: the token is dropped, which
    /// performs no I/O, and the status says the settings were not restored.
    async fn stop_events_inner(&self, restore: bool) -> Option<EventMonitorStatus> {
        let monitor = self.event_monitor.lock().await.take()?;
        let _ = monitor.shutdown_tx.send(true);
        let mut warnings = monitor
            .reader
            .await
            .unwrap_or_else(|error| vec![format!("Event monitoring stopped: {error}")]);
        if restore {
            let saved = monitor.saved;
            match self
                .with_camera(move |connected| Ok(connected.camera.restore_events(saved)))
                .await
            {
                Ok(report) => warnings.extend(viva_zenoh_api::restore_warnings(&report)),
                Err(error) => warnings.push(format!("Event settings were not restored: {error}")),
            }
        } else {
            warnings.push(
                "Event settings were not restored: the camera was lost before monitoring stopped"
                    .into(),
            );
        }
        if !warnings.is_empty() {
            warn!(?warnings, "event monitoring stopped");
        }
        let status = EventMonitorStatus::inactive(warnings);
        self.events.send(&status);
        Some(status)
    }

    /// The reader's channel failed: stop monitoring, unless the session it
    /// belonged to has already been retired, which stopped it.
    async fn end_events(&self, ticket: u64) {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.current(ticket) {
            self.stop_events_inner(true).await;
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
        self.stop_events_inner(true).await;
        let Some(mut connected) = self.camera.lock().await.take() else {
            return;
        };
        let released = tauri::async_runtime::spawn_blocking(move || {
            if !connected.read_only {
                connected.camera.release(RELEASE_ACK_TIMEOUT);
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

impl EmbeddedBackend {
    async fn open(
        &self,
        device_id: &str,
        access: ConnectionAccess,
    ) -> Result<ConnectResult, String> {
        let usb = self.usb.read().await.get(device_id).cloned();
        if let Some(info) = usb {
            if access == ConnectionAccess::Monitor {
                return Err(
                    "Direct monitor access is available only for embedded GigE Vision cameras"
                        .into(),
                );
            }
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
            .as_ref()
            .map(|d| (d.name.clone(), d.model.clone()))
            .unwrap_or_else(|| (device_id.to_string(), String::new()));

        let mut gige_info = viva_genicam::gige::DeviceInfo {
            mac: cached
                .as_ref()
                .and_then(|d| d.mac.as_deref())
                .and_then(parse_mac)
                .unwrap_or_default(),
            local_ip: cached
                .as_ref()
                .and_then(|d| d.mac.as_deref())
                .and_then(parse_mac)
                .and_then(|mac| self.local_ips.try_read().ok()?.get(&mac).copied()),
            model: if model.is_empty() {
                None
            } else {
                Some(model.clone())
            },
            ..viva_genicam::gige::DeviceInfo::from_ip(ip)
        };

        if let Some(found) = viva_genicam::gige::discover_ip(ip, Duration::from_millis(500))
            .await
            .map_err(|e| e.to_string())?
        {
            if gige_info.mac != [0; 6] && gige_info.mac != found.mac {
                return Err("Another camera now occupies the discovered address; rediscover before connecting".into());
            }
            gige_info = found;
        } else if gige_info.mac != [0; 6] {
            let found = viva_genicam::gige::discover_with(
                Duration::from_millis(500),
                &studio_discovery_options(),
            )
            .await
            .map_err(|e| e.to_string())?;
            gige_info = verified_discovery(&gige_info, found)?;
        }
        info!(device_id, "Connecting to GigE camera (embedded mode)");

        let (camera, xml) = viva_genicam::connect_gige_as(&gige_info, access.gige())
            .await
            .map_err(|e| format!("Failed to connect to camera at {device_id}: {e}"))?;

        info!(
            device_id,
            xml_len = xml.len(),
            "Camera connected, XML fetched"
        );

        self.attach(Box::new(camera), device_id, None).await;
        if let Some(connected) = self.camera.lock().await.as_mut() {
            connected.gige_info = Some(gige_info);
            connected.access = access;
            connected.read_only = access == ConnectionAccess::Monitor;
        }

        Ok(ConnectResult {
            xml,
            device_name: name,
            model,
        })
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
        self.connect_session(device_id)
            .await
            .map(|(result, _)| result)
    }

    async fn connect_as(
        &self,
        device_id: &str,
        access: ConnectionAccess,
    ) -> Result<ConnectResult, String> {
        self.connect_session_as(device_id, access)
            .await
            .map(|(result, _)| result)
    }

    async fn connect_model(
        &self,
        device_id: &str,
        access: ConnectionAccess,
        _: &Arc<Connection>,
        _: &SharedModelState,
    ) -> Result<ParseXmlResponse, String> {
        self.connect_session_as(device_id, access)
            .await
            .map(|(_, model)| model)
    }

    async fn disconnect(&self) -> Result<(), String> {
        let ticket = self.lifecycle.begin();
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.current(ticket) {
            self.release_camera().await;
            self.connection
                .set_when(ConnectionState::Disconnected, || {
                    self.lifecycle.current(ticket)
                })
                .await;
        }
        Ok(())
    }

    async fn disconnect_model(&self, _: &Arc<Connection>) -> Result<(), String> {
        self.disconnect().await
    }

    async fn publish_acquisition_status(&self, active: bool, events: &Events) {
        let _transition = self.lifecycle.transition.lock().await;
        let acquisition = self.acquisition.lock().await;
        let running = acquisition
            .as_ref()
            .is_some_and(|s| !s.reader.inner().is_finished());
        if active == running {
            events.send(&if active {
                AcquisitionStatus::active(None, None)
            } else {
                AcquisitionStatus::inactive()
            });
        }
    }

    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let name = name.to_string();
        self.with_camera(move |connected| {
            let mut state = connected.camera.feature_state(&name)?;
            if !connected.writes_allowed() {
                cap_read_only(&mut state);
            }
            Ok(state)
        })
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
            if !connected.writes_allowed() {
                return Err("Camera control lost: reconnect explicitly to write".into());
            }
            connected.camera.set(&name, &value)
        })
        .await
    }

    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String> {
        let name = name.to_string();
        self.with_camera(move |connected| {
            if !connected.writes_allowed() {
                return Err("Camera control lost: reconnect explicitly to execute commands".into());
            }
            connected.camera.execute(&name)
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
                match connected.camera.feature_state(&name) {
                    Ok(mut state) => {
                        if !connected.writes_allowed() {
                            cap_read_only(&mut state);
                        }
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
        let ticket = self.lifecycle.ticket();
        let _transition = self.lifecycle.transition.lock().await;
        if self
            .lifecycle
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
            || !self.lifecycle.current(ticket)
        {
            return Err("Connection superseded or Studio is shutting down".into());
        }
        // Stop any previous acquisition.
        self.stop_acquisition_inner().await;

        let (width, height, pixel_format, frames) = self
            .with_camera(move |connected| {
                if connected.access != ConnectionAccess::Monitor && !connected.writes_allowed() {
                    return Err(
                        "Camera control lost: reconnect explicitly before starting acquisition"
                            .into(),
                    );
                }
                let (width, height, pixel_format) = connected.camera.image_format();
                let frames = connected.camera.start(&options)?;
                Ok((width, height, pixel_format, frames))
            })
            .await?;

        // Create WS broadcast channels.
        let (frame_tx, _) = watch::channel(Bytes::new());
        let native = viva_streamer::native::NativeCapture::default();
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

        let viewer = viva_streamer::ws::AppState {
            frame_tx,
            info_tx,
            native,
        };
        let frame_reader_handle = tauri::async_runtime::spawn(run_frame_reader(
            frames,
            viewer.clone(),
            shutdown_rx.clone(),
            StreamInfoTracker::new(width, height, pixel_format),
            self.events.clone(),
            Some((self.clone(), ticket)),
        ));

        // Spawn WS server task.
        let ws_handle = {
            let state = viewer;
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

        if !self.lifecycle.current(ticket) {
            self.stop_acquisition_inner().await;
            return Err("Connection superseded".into());
        }
        Ok(StreamerInfo {
            ws_url,
            width,
            height,
        })
    }

    async fn stop_acquisition(&self) -> Result<(), String> {
        let _transition = self.lifecycle.transition.lock().await;
        self.stop_acquisition_inner().await;
        // Stop acquisition on the camera, if one is still connected.
        match self
            .with_camera(|connected| {
                if !connected.writes_allowed() {
                    Ok(())
                } else {
                    connected.camera.acquisition_stop()
                }
            })
            .await
        {
            Err(e) if e == "No camera connected" => Ok(()),
            other => other,
        }
    }

    async fn start_events(&self, names: Vec<String>) -> Result<EventMonitorStatus, String> {
        if names.is_empty() {
            return Err("Choose at least one event to monitor".into());
        }
        let ticket = self.lifecycle.ticket();
        let _transition = self.lifecycle.transition.lock().await;
        if self
            .lifecycle
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
            || !self.lifecycle.current(ticket)
        {
            return Err("Connection superseded or Studio is shutting down".into());
        }
        let previous = self.stop_events_inner(true).await;
        let chosen = names.clone();
        let (events, saved) = self
            .with_camera(move |connected| {
                if !connected.writes_allowed() {
                    return Err(
                        "Camera control lost or read-only: event settings cannot be changed".into(),
                    );
                }
                connected.camera.start_events(&chosen)
            })
            .await?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let reader = tauri::async_runtime::spawn(run_event_reader(
            events,
            shutdown_rx,
            self.clone(),
            ticket,
        ));
        *self.event_monitor.lock().await = Some(EventMonitor {
            shutdown_tx,
            reader,
            saved,
        });
        if !self.lifecycle.current(ticket) {
            self.stop_events_inner(true).await;
            return Err("Connection superseded".into());
        }
        info!(?names, "Event monitoring started (embedded mode)");
        let mut status = EventMonitorStatus::active(names);
        // What stopping the previous monitoring could not undo.
        status.warnings = previous.map(|p| p.warnings).unwrap_or_default();
        self.events.send(&status);
        Ok(status)
    }

    async fn stop_events(&self) -> Result<EventMonitorStatus, String> {
        let _transition = self.lifecycle.transition.lock().await;
        Ok(match self.stop_events_inner(true).await {
            Some(status) => status,
            None => {
                let status = EventMonitorStatus::inactive(Vec::new());
                self.events.send(&status);
                status
            }
        })
    }

    async fn save_configuration(&self) -> Result<ConfigurationSaved, String> {
        self.with_camera(|connected| {
            if !connected.writes_allowed() {
                return Err(
                    "Camera control lost or read-only: saving a configuration sets selectors to read each entry"
                        .into(),
                );
            }
            let configuration = connected.camera.save_configuration()?;
            Ok(ConfigurationSaved::from(&configuration))
        })
        .await
    }

    async fn load_configuration(&self, text: String) -> Result<ConfigurationLoaded, String> {
        let configuration = viva_genicam::genapi::Configuration::parse(&text)
            .map_err(|e| format!("Not a configuration file: {e}"))?;
        self.with_camera(move |connected| {
            if !connected.writes_allowed() {
                return Err(
                    "Camera control lost or read-only: a configuration cannot be loaded".into(),
                );
            }
            let report = connected.camera.load_configuration(&configuration);
            Ok(ConfigurationLoaded::from(&report))
        })
        .await
    }

    async fn read_register(&self, name: &str) -> Result<Vec<u8>, String> {
        let name = name.to_string();
        self.with_camera(move |connected| connected.camera.get_register(&name))
            .await
    }

    async fn write_register(&self, name: &str, data: Vec<u8>) -> Result<Vec<String>, String> {
        let name = name.to_string();
        self.with_camera(move |connected| {
            if !connected.writes_allowed() {
                return Err("Camera control lost: reconnect explicitly to write".into());
            }
            connected.camera.set_register(&name, &data)
        })
        .await
    }

    async fn default_action_destination(&self) -> Result<Ipv4Addr, String> {
        let info = self
            .with_camera(|connected| Ok(connected.gige_info.clone()))
            .await
            .ok()
            .flatten();
        Ok(info
            .as_ref()
            .and_then(subnet_broadcast)
            .unwrap_or(Ipv4Addr::BROADCAST))
    }

    async fn send_action(
        &self,
        destination: Ipv4Addr,
        params: viva_genicam::gige::ActionParams,
        timeout_ms: u64,
    ) -> Result<viva_genicam::gige::AckSummary, String> {
        let address = std::net::SocketAddr::new(destination.into(), viva_genicam::gige::GVCP_PORT);
        let summary = viva_genicam::gige::send_action(address, &params, timeout_ms)
            .await
            .map_err(|e| format!("Sending the action command to {destination} failed: {e}"))?;
        info!(%destination, acks = summary.acks, refusals = summary.refusals.len(), "Action command sent");
        Ok(summary)
    }

    async fn shutdown(&self) {
        self.lifecycle
            .shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        self.lifecycle.begin();
        let _transition = self.lifecycle.transition.lock().await;
        self.release_camera().await;
        self.connection.set(ConnectionState::Disconnected).await;
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
            if !connected.writes_allowed() {
                return Err("Camera control lost: persistent IP changes require control".into());
            }
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

/// The broadcast address of the network a camera was found on: the host
/// side's, which the broadcast leaves from, or else the camera's own. `None`
/// without an address and mask, or for a host route (`/31`, `/32`), which has
/// no broadcast address.
fn subnet_broadcast(info: &viva_genicam::gige::DeviceInfo) -> Option<Ipv4Addr> {
    let (ip, mask) = match (info.local_ip, info.local_netmask) {
        (Some(ip), Some(mask)) => (ip, mask),
        _ => (info.ip, info.subnet?),
    };
    let mask = u32::from(mask);
    if mask.count_ones() >= 31 || ip.is_unspecified() {
        return None;
    }
    Some(Ipv4Addr::from(u32::from(ip) | !mask))
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

impl EmbeddedBackend {
    async fn connect_session(
        &self,
        device_id: &str,
    ) -> Result<(ConnectResult, ParseXmlResponse), String> {
        self.connect_session_as(device_id, ConnectionAccess::Control)
            .await
    }

    async fn connect_session_as(
        &self,
        device_id: &str,
        access: ConnectionAccess,
    ) -> Result<(ConnectResult, ParseXmlResponse), String> {
        if self
            .lifecycle
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err("Studio is shutting down".into());
        }
        if access == ConnectionAccess::Monitor && self.usb.read().await.contains_key(device_id) {
            return Err(
                "Direct monitor access is available only for embedded GigE Vision cameras".into(),
            );
        }
        let ticket = self.lifecycle.begin();
        let mut cancelled = self.lifecycle.generation.subscribe();
        let _transition = self.lifecycle.transition.lock().await;
        if self
            .lifecycle
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
            || !self.lifecycle.current(ticket)
        {
            return Err("Connection superseded or Studio is shutting down".into());
        }
        self.release_camera().await;
        self.connection
            .set_when(
                ConnectionState::Connecting {
                    device_id: device_id.into(),
                },
                || self.lifecycle.current(ticket),
            )
            .await;
        // A USB open is blocking and must finish before a replacement opens its
        // endpoint. GigE opens are cancel-safe, including claimed-control cleanup.
        let result = if self.usb.read().await.contains_key(device_id) {
            self.open(device_id, access).await
        } else {
            tokio::select! {
                biased;
                _ = cancelled.changed() => Err("Connection superseded".into()),
                opened = self.open(device_id, access) => opened,
            }
        };
        if !self.lifecycle.current(ticket) {
            self.release_camera().await;
            return Err("Connection superseded".into());
        }
        let result = match result {
            Ok(result) => result,
            Err(message) => {
                self.connection
                    .set_when(
                        ConnectionState::Error {
                            message: message.clone(),
                        },
                        || self.lifecycle.current(ticket),
                    )
                    .await;
                return Err(message);
            }
        };
        let parsed = match parse_camera_model(result.xml.clone()) {
            Ok(parsed) => parsed,
            Err(message) => {
                self.release_camera().await;
                self.connection
                    .set_when(
                        ConnectionState::Error {
                            message: message.clone(),
                        },
                        || self.lifecycle.current(ticket),
                    )
                    .await;
                return Err(message);
            }
        };
        self.publish_model(
            ticket,
            device_id,
            &result.device_name,
            &result.model,
            &parsed,
            false,
        )
        .await?;
        self.watch_session(ticket);
        Ok((result, parsed))
    }

    async fn publish_model(
        &self,
        ticket: u64,
        device_id: &str,
        name: &str,
        model_name: &str,
        parsed: &ParseXmlResponse,
        replacement: bool,
    ) -> Result<(), String> {
        let access = self.camera.lock().await.as_ref().map(|c| c.access);
        let mut model = self.model.write().await;
        if !self.lifecycle.current(ticket) {
            return Err("Connection superseded".into());
        }
        model.update(
            parsed.xml.clone(),
            parsed.graph.clone(),
            parsed.diags.clone(),
            Some(device_id.into()),
        );
        drop(model);
        self.connection
            .set_when(
                ConnectionState::Connected {
                    device_id: device_id.into(),
                    device_name: name.into(),
                    model: model_name.into(),
                    access,
                },
                || self.lifecycle.current(ticket),
            )
            .await;
        if !self.lifecycle.current(ticket) {
            return Err("Connection superseded".into());
        }
        if replacement {
            self.events.send(&DeviceReconnected {
                device_id: device_id.into(),
                node_count: parsed.summary.node_count,
                model: parsed.clone(),
            });
        }
        Ok(())
    }

    fn watch_session(&self, ticket: u64) {
        let backend = self.clone();
        let mut cancelled = self.lifecycle.generation.subscribe();
        tauri::async_runtime::spawn(async move {
            let mut control = backend
                .with_camera(|c| Ok(c.camera.watch_control()))
                .await
                .ok()
                .flatten();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = cancelled.changed() => return,
                    _ = async {
                        match &mut control {
                            Some(control) => { if control.changed().await.is_err() { std::future::pending::<()>().await; } },
                            None => std::future::pending::<()>().await,
                        }
                    } => {},
                    _ = tick.tick() => {},
                }
                if !backend.lifecycle.current(ticket) {
                    return;
                }
                match backend.with_camera(|c| c.camera.health()).await {
                    Err(reason) => {
                        backend.lose_camera(ticket, reason).await;
                        return;
                    }
                    Ok(ControlState::Lost) => backend.lose_control(ticket).await,
                    _ => {}
                }
            }
        });
    }

    async fn lose_control(&self, ticket: u64) {
        let _transition = self.lifecycle.transition.lock().await;
        if !self.lifecycle.current(ticket) {
            return;
        }
        let Ok(Some(device_id)) = self
            .with_camera(|c| {
                if c.read_only {
                    return Ok(None);
                }
                c.read_only = true;
                Ok(Some(c.device_id.clone()))
            })
            .await
        else {
            return;
        };
        self.stop_acquisition_inner().await;
        // Restoration is skipped without control, and the status says so.
        self.stop_events_inner(true).await;
        let previous = self.connection.get().await;
        let (device_name, model) = match previous {
            ConnectionState::Connected {
                device_name, model, ..
            } => (device_name, model),
            _ => (device_id.clone(), String::new()),
        };
        self.connection.set_when(ConnectionState::ControlLost { device_id, device_name, model, reason: "Camera control was lost. Features are read-only; reconnect explicitly to claim control again.".into() }, || self.lifecycle.current(ticket)).await;
        if self.lifecycle.current(ticket) {
            let names = self
                .model
                .read()
                .await
                .graph
                .as_ref()
                .map(|g| g.nodes_by_name.keys().cloned().collect())
                .unwrap_or_default();
            self.events.send(&FeaturesInvalidated {
                cause: "control-lost".into(),
                names,
            });
        }
    }

    async fn lose_camera(&self, ticket: u64, reason: String) {
        let _transition = self.lifecycle.transition.lock().await;
        if !self.lifecycle.current(ticket) {
            return;
        }
        let Some((device_id, previous, was_read_only, access)) =
            self.camera.lock().await.as_ref().map(|c| {
                (
                    c.device_id.clone(),
                    c.gige_info.clone(),
                    !c.writes_allowed(),
                    c.access,
                )
            })
        else {
            return;
        };
        // Retire every notification from the lost session before recovery.
        // A reader error and a watchdog timeout may arrive together.
        let Some(ticket) = self.lifecycle.advance(ticket) else {
            return;
        };
        self.events.send(&DisconnectReason {
            device_id: device_id.clone(),
            message: reason.clone(),
        });
        self.connection
            .set_when(
                ConnectionState::Reconnecting {
                    device_id: device_id.clone(),
                    attempt: 0,
                    max_attempts: 1,
                    reason: reason.clone(),
                },
                || self.lifecycle.current(ticket),
            )
            .await;
        self.stop_acquisition_inner().await;
        // No restoring writes on a missing link, as for AcquisitionStop.
        self.stop_events_inner(false).await;
        if let Some(connected) = self.camera.lock().await.take() {
            // The failed health transaction already exhausted its retries.
            // Drop cancels/drains the keepalive and makes one bounded release;
            // do not traverse AcquisitionStop/TLParamsLocked on a missing link.
            tauri::async_runtime::spawn_blocking(move || drop(connected))
                .await
                .ok();
        }
        let Some(previous) = previous
            .filter(|p| p.mac != [0; 6] && (!was_read_only || access == ConnectionAccess::Monitor))
        else {
            self.connection
                .set_when(
                    ConnectionState::Error {
                        message: format!("{reason}. Reconnect manually; automatic recovery requires a verified GigE identity and a session that still held control."),
                    },
                    || self.lifecycle.current(ticket),
                )
                .await;
            return;
        };
        // Studio has no explicitly selected NIC. The old endpoint is tried
        // first, then every physical interface can rediscover the same MAC.
        let options = Reopen::new()
            .patience(self.reopen_patience)
            .access(access.gige());
        #[cfg(test)]
        let options = match &self.reopen_interface {
            Some(interface) => options.interface(interface),
            None => options,
        };
        let mut cancelled = self.lifecycle.generation.subscribe();
        if !self.lifecycle.current(ticket) {
            return;
        }
        let reopened = tokio::select! {
            biased;
            _ = cancelled.changed() => return,
            result = viva_genicam::reopen_gige(&previous, &options) => result,
        };
        if !self.lifecycle.current(ticket) {
            if let Ok(reopened) = reopened {
                tauri::async_runtime::spawn_blocking(move || drop(reopened))
                    .await
                    .ok();
            }
            return;
        }
        match reopened {
            Ok(reopened) => {
                // Recovery never resumes an old acquisition, including a camera
                // that kept sending while only the host link was interrupted.
                let stopped = tauri::async_runtime::spawn_blocking(move || {
                    let mut reopened = reopened;
                    if access == ConnectionAccess::Monitor {
                        Ok(reopened)
                    } else {
                        reopened.camera.acquisition_stop().map(|()| reopened)
                    }
                })
                .await;
                let reopened = match stopped {
                    Ok(Ok(reopened)) => reopened,
                    Ok(Err(error)) => {
                        self.connection.set_when(ConnectionState::Error { message: format!("The camera reopened but acquisition could not be stopped: {error}") }, || self.lifecycle.current(ticket)).await;
                        return;
                    }
                    Err(error) => {
                        self.connection
                            .set_when(
                                ConnectionState::Error {
                                    message: format!(
                                        "Stopping the reopened camera failed: {error}"
                                    ),
                                },
                                || self.lifecycle.current(ticket),
                            )
                            .await;
                        return;
                    }
                };
                if !self.lifecycle.current(ticket) {
                    tauri::async_runtime::spawn_blocking(move || drop(reopened))
                        .await
                        .ok();
                    return;
                }
                let parsed = match parse_camera_model(reopened.xml.clone()) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        tauri::async_runtime::spawn_blocking(move || drop(reopened))
                            .await
                            .ok();
                        self.connection
                            .set_when(ConnectionState::Error { message }, || {
                                self.lifecycle.current(ticket)
                            })
                            .await;
                        return;
                    }
                };
                let info = reopened.device;
                let name = info
                    .user_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .or_else(|| info.model.clone())
                    .unwrap_or_else(|| device_id.clone());
                let model_name = info.model.clone().unwrap_or_default();
                self.attach(Box::new(reopened.camera), &device_id, None)
                    .await;
                if let Some(connected) = self.camera.lock().await.as_mut() {
                    connected.gige_info = Some(info.clone());
                    connected.access = access;
                    connected.read_only = access == ConnectionAccess::Monitor;
                }
                if let Some(local) = info.local_ip {
                    self.local_ips.write().await.insert(info.mac, local);
                }
                let (devices, _) = discovered_gige(vec![info]);
                let mut discovered = self.discovered.write().await;
                if !self.lifecycle.current(ticket) {
                    return;
                }
                for device in devices {
                    if let Some(cached) = discovered.iter_mut().find(|d| d.id == device.id) {
                        *cached = device.clone();
                    } else {
                        discovered.push(device.clone());
                    }
                    self.events.send(&device);
                }
                drop(discovered);
                if self
                    .publish_model(ticket, &device_id, &name, &model_name, &parsed, true)
                    .await
                    .is_ok()
                {
                    self.watch_session(ticket);
                }
            }
            Err(error) => {
                self.connection
                    .set_when(
                        ConnectionState::Error {
                            message: format!(
                                "Could not reconnect within {:?}: {error}. Reconnect manually.",
                                self.reopen_patience
                            ),
                        },
                        || self.lifecycle.current(ticket),
                    )
                    .await
            }
        }
    }
}

// ── Streaming ───────────────────────────────────────────────────────────────

/// Read frames until shutdown, sending the viewer a BMP of each as often as
/// [`FramePacer`] allows: a cadence of [`DISPLAY_FPS`] a second, and once a second
/// while no viewer is connected, so one that connects later has a frame.
/// Every [`STATUS_INTERVAL`] it also reports the stream's frame rate and
/// counters, whether or not frames arrive, and on the way out the final
/// counters as [`AcquisitionStatus::stopped`], as `viva-service` does (ST-36).
async fn run_frame_reader(
    mut frame_stream: Frames,
    viewer: viva_streamer::ws::AppState,
    mut shutdown_rx: watch::Receiver<bool>,
    mut info: StreamInfoTracker,
    events: Arc<Events>,
    lifecycle: Option<(EmbeddedBackend, u64)>,
) {
    let viva_streamer::ws::AppState {
        frame_tx,
        native,
        info_tx,
    } = viewer;
    let mut terminal_error = None;
    let mut pacer = FramePacer::new(Duration::from_secs(1) / DISPLAY_FPS);
    let mut last_status = tokio::time::Instant::now();
    let mut status_tick = tokio::time::interval_at(last_status + STATUS_INTERVAL, STATUS_INTERVAL);
    status_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
            _ = status_tick.tick() => {
                let now = tokio::time::Instant::now();
                events.send(&AcquisitionStatus::active(
                    Some(frames_since_status as f32 / (now - last_status).as_secs_f32()),
                    Some(StreamCounters::from(&frame_stream.stats())),
                ));
                last_status = now;
                frames_since_status = 0;
                continue;
            }
            // Cancel-safe for both transports: a tick loses no frame.
            result = frame_stream.next_frame() => match result {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    info!("Frame stream ended");
                    break;
                }
                Err(e) => {
                    warn!("Frame stream error: {e}");
                    terminal_error = Some(e.to_string());
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
        let admitted_at = Instant::now();
        if !pacer.is_due(frame_tx.receiver_count() > 0, admitted_at) {
            continue;
        }
        if !FrameEncoder::can_draw(frame.pixel_format) {
            if unsupported != Some(frame.pixel_format) {
                warn!(pixel_format = ?frame.pixel_format, "Pixel format cannot be drawn; dropping its frames");
                unsupported = Some(frame.pixel_format);
            }
            continue;
        }
        match encoder.encode(
            frame.pixel_format,
            frame.width,
            frame.height,
            frame.row_stride(),
            frame.payload.as_ref(),
        ) {
            Ok(bmp) => {
                if *shutdown_rx.borrow()
                    || lifecycle
                        .as_ref()
                        .is_some_and(|(backend, ticket)| !backend.lifecycle.current(*ticket))
                {
                    break;
                }
                let mut metadata = viva_zenoh_api::FrameMetadata::from(&frame);
                // Chunk values are decoded only for preview-admitted frames.
                // Plain frames never lock or read the camera for metadata.
                if frame.chunks.is_some()
                    && let Some((backend, ticket)) = lifecycle.as_ref()
                {
                    let frame = frame.clone();
                    let current = backend.lifecycle.clone();
                    let ticket = *ticket;
                    match backend
                        .with_camera(move |connected| {
                            if !current.current(ticket) {
                                return Err("Frame belongs to an earlier camera session".into());
                            }
                            Ok(connected.camera.chunk_features(&frame))
                        })
                        .await
                    {
                        Ok(features) => metadata = metadata.with_features(features),
                        Err(error) => {
                            metadata = metadata
                                .with_features([("ChunkData".into(), Err::<String, _>(error))])
                        }
                    }
                }
                if *shutdown_rx.borrow()
                    || lifecycle
                        .as_ref()
                        .is_some_and(|(backend, ticket)| !backend.lifecycle.current(*ticket))
                {
                    break;
                }
                let metadata = match viva_streamer::preview::normalize(metadata) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        warn!(%error, "Invalid frame metadata");
                        continue;
                    }
                };
                let preview = match viva_streamer::preview::encode(&bmp, Some(&metadata)) {
                    Ok(preview) => preview,
                    Err(error) => {
                        warn!(%error, "Frame metadata not drawn");
                        continue;
                    }
                };
                native.replace(viva_streamer::native::SourceFrame {
                    format: frame.pixel_format,
                    width: frame.width,
                    height: frame.height,
                    row_stride: frame.row_stride(),
                    pixels: frame.payload.clone(),
                    bmp: bmp.clone(),
                    metadata: Some(metadata),
                });
                // Stored even with no viewer: the next one to connect is sent it.
                frame_tx.send_replace(preview);
                pacer.sent(admitted_at);
            }
            Err(e) => warn!("Frame not drawn: {e}"),
        }
    }
    // The final counters, sent before the stop command's `inactive`: the
    // backend awaits this task first.
    events.send(&AcquisitionStatus::stopped(StreamCounters::from(
        &frame_stream.stats(),
    )));
    // Wait for the stream to let go of the camera before this task ends: the
    // backend awaits this task before it starts another stream or releases
    // the camera.
    frame_stream.shutdown().await;
    if *shutdown_rx.borrow() {
        terminal_error = None;
    }
    if let (Some(reason), Some((backend, ticket))) = (terminal_error, lifecycle) {
        // Separate task: loss handling drains this reader before releasing the camera.
        tauri::async_runtime::spawn(async move {
            backend.lose_camera(ticket, reason).await;
        });
    }
}

/// Forward each event, with the features its XML reads from it, until
/// shutdown or until the channel fails; then shut the channel down and
/// return what that could not undo. A failed channel stops monitoring through
/// [`EmbeddedBackend::end_events`], on a task of its own: stopping joins this
/// reader.
async fn run_event_reader(
    events: CameraEvents,
    mut shutdown_rx: watch::Receiver<bool>,
    backend: EmbeddedBackend,
    ticket: u64,
) -> Vec<String> {
    let blocks = events.carry_blocks();
    let mut warnings = Vec::new();
    let mut failed = false;
    loop {
        let event = tokio::select! {
            _ = shutdown_rx.wait_for(|stop| *stop) => break,
            event = events.next() => match event {
                Ok(event) => event,
                Err(error) => {
                    warn!("Event channel error: {error}");
                    warnings.push(format!("Event monitoring stopped: {error}"));
                    failed = true;
                    break;
                }
            },
        };
        let wire = CameraEvent::new(&event, !blocks);
        let current = backend.lifecycle.clone();
        let read = backend
            .with_camera(move |connected| {
                if !current.current(ticket) {
                    return Err("Event belongs to an earlier camera session".into());
                }
                Ok(connected.camera.read_event(&event))
            })
            .await;
        if *shutdown_rx.borrow() || !backend.lifecycle.current(ticket) {
            break;
        }
        let wire = match read {
            Ok((name, features)) => wire.with_name(name).with_features(features),
            Err(error) => wire.with_features([("Event".to_string(), Err::<String, _>(error))]),
        };
        backend.events.send(&wire);
    }
    if let Err(error) = events.shutdown().await {
        warnings.push(error);
    }
    if failed && !*shutdown_rx.borrow() {
        let backend = backend.clone();
        tauri::async_runtime::spawn(async move { backend.end_events(ticket).await });
    }
    warnings
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

// ── Discovery helpers ───────────────────────────────────────────────────────

fn studio_discovery_options() -> viva_genicam::gige::DiscoveryOptions {
    // Defaults exclude loopback; broadcast ACKs expose devices on other subnets.
    viva_genicam::gige::DiscoveryOptions::new().allow_broadcast_ack(true)
}

fn verified_discovery(
    previous: &viva_genicam::gige::DeviceInfo,
    found: Vec<viva_genicam::gige::DeviceInfo>,
) -> Result<viva_genicam::gige::DeviceInfo, String> {
    found.into_iter().find(|d| d.mac == previous.mac && d.ip == previous.ip)
        .ok_or_else(|| "The discovered camera's MAC could not be verified at its address; rediscover before connecting".into())
}

async fn discover_gige_devices() -> Vec<viva_genicam::gige::DeviceInfo> {
    let timeout = Duration::from_secs(1);
    // `discover` rather than `discover_all`: the latter probes loopback, which
    // on Windows can raise WSAECONNRESET and used to abort the whole scan,
    // leaving the UI on "No device" with a real camera on the wire (#57).
    // `discover_all` remains for the fake-camera tests that need loopback.
    viva_genicam::gige::discover_with(timeout, &studio_discovery_options())
        .await
        .unwrap_or_else(|e| {
            warn!("GigE discovery failed: {e}");
            Vec::new()
        })
}

/// USB3 Vision cameras on this host. USB enumeration blocks, so on the
/// blocking pool; a host without USB access finds none.
async fn discover_u3v_devices() -> Vec<U3vDeviceInfo> {
    let found = tauri::async_runtime::spawn_blocking(viva_genicam::u3v::discover).await;
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UsbPosition {
    bus: String,
    address: u8,
    vendor_id: u16,
    product_id: u16,
}

impl UsbPosition {
    fn of(info: &U3vDeviceInfo) -> Self {
        Self {
            bus: info.bus.clone(),
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
            let mut info = DeviceInfo::from_announce(announce, "gige");
            info.shares_subnet_with_host = d.shares_subnet_with_host();
            info
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

    use viva_fake_gige::REG_ACQ_STOP;
    use viva_fake_gige::{FakeCamera, GvcpCommand};
    use viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE as CCP;

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
        first_preview(ws_url).await.1
    }

    pub(crate) async fn first_preview(
        ws_url: &str,
    ) -> (Option<viva_zenoh_api::FrameMetadata>, Vec<u8>) {
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
        let (metadata, bmp) =
            viva_streamer::preview::decode(&frame).expect("valid preview envelope");
        (metadata, bmp.to_vec())
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

    /// Teardown releases UDP 3956 before another test can acquire the guard.
    #[test]
    fn fake_shutdown_allows_immediate_rebind() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            for _ in 0..8 {
                let fake = viva_fake_gige::FakeCamera::builder()
                    .bind_ip([127, 0, 0, 1].into())
                    .port(3956)
                    .build()
                    .await
                    .expect("previous fake released UDP 3956");
                fake.stop().await;
            }
        });
    }

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
            fake.stop().await;
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
            _fake.stop().await;
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
            _fake.stop().await;
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
            _fake.stop().await;
        });
    }

    /// While streaming, the backend reports the camera's frame rate and the
    /// library's stream counters once a second. Embedded mode reported
    /// neither, and the header showed only "Acquiring" (ST-36). When the
    /// stream stops, the final counters follow, so the UI keeps them.
    #[test]
    fn streaming_reports_the_frame_rate_and_the_stream_counters() {
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
                        s["active"] == true
                            && s["fps"].as_f64().is_some_and(|fps| fps > 0.0)
                            && s["stats"]["frames_complete"]
                                .as_u64()
                                .is_some_and(|n| n > 0)
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
            let stats = &status["stats"];
            for lost in ["frames_incomplete", "frames_missed", "frames_backpressure"] {
                assert_eq!(stats[lost], 0, "{status}");
            }
            assert!(stats["packets_received"].as_u64() > Some(0), "{status}");

            backend.stop_acquisition().await.expect("stop");
            let last = recorder
                .of("acquisition-status")
                .pop()
                .expect("a status after stop");
            assert_eq!(last["active"], false, "{last}");
            assert!(
                last["stats"]["frames_complete"].as_u64() >= stats["frames_complete"].as_u64(),
                "the stopped status carries the final counters: {last}"
            );
            backend.disconnect().await.expect("disconnect");
            _fake.stop().await;
        });
    }

    /// The device panel's IP configuration (ST-37): the backend reads the
    /// camera's current and persistent address, and writes a persistent one
    /// the camera keeps. Tauri refused these commands until #175 granted
    /// them, so nothing had run this path.
    #[test]
    fn network_config_reads_and_writes_the_persistent_address() {
        use viva_genicam::gige::consts::{
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
            fake.stop().await;
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
        use viva_genicam::u3v::U3vDeviceInfo;
        let info = |serial: Option<&str>| U3vDeviceInfo {
            bus: "2".to_string(),
            address: 7,
            vendor_id: 0x2676,
            product_id: 0xba02,
            serial: serial.map(str::to_string),
            manufacturer: Some("Vendor".to_string()),
            model: Some("Model".to_string()),
            speed: None,
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

    fn u3v_info(serial: Option<&str>) -> viva_genicam::u3v::U3vDeviceInfo {
        viva_genicam::u3v::U3vDeviceInfo {
            bus: "2".to_string(),
            address: 7,
            vendor_id: 0x2676,
            product_id: 0xba02,
            serial: serial.map(str::to_string),
            manufacturer: Some("Vendor".to_string()),
            model: Some("Model".to_string()),
            speed: None,
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
        use viva_genicam::u3v::UsbTransfer;

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
            let device = viva_fake_u3v::open(transport.clone()).expect("open");
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
        use viva_genicam::gige::consts::{
            STREAM_CHANNEL_BASE, STREAM_PACKET_SIZE as SCP_PACKET_SIZE,
        };
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
            fake.stop().await;
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
            _fake.stop().await;
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::state::events::Recorder;
    use viva_fake_gige::{GvcpCommand, REG_ACQ_START};

    async fn wait_state(backend: &EmbeddedBackend, predicate: impl Fn(&ConnectionState) -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if predicate(&backend.connection.get().await) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("expected connection transition");
    }

    #[test]
    fn generation_cancels_old_work_and_stale_model_publication() {
        tauri::async_runtime::block_on(async {
            let backend = EmbeddedBackend::new(Default::default());
            let ticket = backend.lifecycle.begin();
            let mut cancellation = backend.lifecycle.generation.subscribe();
            assert!(backend.lifecycle.current(ticket));
            backend.lifecycle.begin();
            cancellation.changed().await.expect("cancelled");
            assert!(!backend.lifecycle.current(ticket));
            let parsed = parse_camera_model(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../../crates/viva_xml_model/fixtures/minimal.xml"
                ))
                .into(),
            )
            .expect("model");
            assert!(
                backend
                    .publish_model(ticket, "stale", "old", "old", &parsed, true)
                    .await
                    .is_err()
            );
            assert!(backend.model.read().await.xml.is_none());
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Disconnected
            ));
        });
    }

    #[test]
    fn explicit_disconnect_wins_when_loss_waits_for_the_camera_mutex() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let (events, recorder) = Recorder::events();
            let backend = EmbeddedBackend::new(events);
            backend.connect("127.0.0.1").await.expect("connect");
            let camera = backend.camera.lock().await;
            let ticket = backend.lifecycle.ticket();
            let losing = backend.clone();
            let loss = tauri::async_runtime::spawn(async move {
                losing.lose_camera(ticket, "late loss".into()).await;
            });
            while backend.lifecycle.transition.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
            let closing = backend.clone();
            let disconnect = tauri::async_runtime::spawn(async move {
                closing.disconnect().await.expect("disconnect");
            });
            while backend.lifecycle.current(ticket) {
                tokio::task::yield_now().await;
            }
            drop(camera);
            loss.await.expect("loss task");
            disconnect.await.expect("disconnect task");
            assert!(backend.camera.lock().await.is_none());
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Disconnected
            ));
            assert!(recorder.of("disconnect-reason").is_empty());
            assert!(recorder.of("device-reconnected").is_empty());
            fake.stop().await;
        });
    }

    #[test]
    fn exclusive_takeover_is_control_loss_rather_than_a_reclaim_loop() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let (events, recorder) = Recorder::events();
            let backend = EmbeddedBackend::new(events);
            backend.connect("127.0.0.1").await.expect("connect");
            fake.revoke_control().await;
            let info = backend
                .camera
                .lock()
                .await
                .as_ref()
                .unwrap()
                .gige_info
                .clone()
                .unwrap();
            let (other, _) = viva_genicam::connect_gige_as(&info, viva_genicam::Access::Exclusive)
                .await
                .expect("other controller");
            let owner = fake.controller().await.expect("other owns camera");
            wait_state(&backend, |s| {
                matches!(s, ConnectionState::ControlLost { .. })
            })
            .await;
            fake.commands().reset();
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(backend.start_acquisition(Default::default()).await.is_err());
            assert_eq!(fake.controller().await, Some(owner));
            assert_eq!(
                fake.commands().total(GvcpCommand::WriteReg)
                    + fake.commands().total(GvcpCommand::WriteMem),
                0,
                "no automatic claims or feature writes"
            );
            assert!(recorder.of("device-reconnected").is_empty());
            backend.disconnect().await.expect("disconnect");
            assert_eq!(
                fake.controller().await,
                Some(owner),
                "disconnect cannot release the other controller"
            );
            tauri::async_runtime::spawn_blocking(move || drop(other))
                .await
                .expect("release other");
            fake.stop().await;
        });
    }

    #[test]
    fn usb_loss_is_detected_idle_and_streaming_without_automatic_reopen() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        use viva_genicam::u3v::{U3vError, UsbTransfer};
        struct Disconnecting {
            inner: viva_fake_u3v::FakeU3vTransport,
            unplugged: AtomicBool,
        }
        impl UsbTransfer for Disconnecting {
            fn bulk_write(
                &self,
                ep: u8,
                bytes: &[u8],
                timeout: Duration,
            ) -> Result<usize, U3vError> {
                if self.unplugged.load(SeqCst) {
                    return Err(U3vError::Disconnected);
                }
                self.inner.bulk_write(ep, bytes, timeout)
            }
            fn bulk_read(
                &self,
                ep: u8,
                bytes: &mut [u8],
                timeout: Duration,
            ) -> Result<usize, U3vError> {
                if self.unplugged.load(SeqCst) {
                    return Err(U3vError::Disconnected);
                }
                self.inner.bulk_read(ep, bytes, timeout)
            }
        }
        tauri::async_runtime::block_on(async {
            for streaming in [false, true] {
                let transport = Arc::new(Disconnecting {
                    inner: viva_fake_u3v::FakeU3vTransport::new(64, 48, 0x0108_0001),
                    unplugged: AtomicBool::new(false),
                });
                let device = viva_fake_u3v::open(transport.clone()).expect("fake USB");
                let (camera, _) = viva_genicam::open_u3v_device(device).expect("camera");
                let backend = EmbeddedBackend::new(Default::default());
                backend.attach(Box::new(camera), "usb-test", None).await;
                backend
                    .connection
                    .set(ConnectionState::Connected {
                        device_id: "usb-test".into(),
                        device_name: "USB".into(),
                        model: "USB".into(),
                        access: Some(ConnectionAccess::Control),
                    })
                    .await;
                backend.watch_session(backend.lifecycle.ticket());
                if streaming {
                    let stream = backend
                        .start_acquisition(Default::default())
                        .await
                        .expect("start");
                    fake_camera::first_frame(&stream.ws_url).await;
                }
                transport.unplugged.store(true, SeqCst);
                wait_state(&backend, |s| matches!(s, ConnectionState::Error { .. })).await;
                assert!(backend.camera.lock().await.is_none());
                assert!(backend.acquisition.lock().await.is_none());
                backend.disconnect().await.expect("cleanup");
            }
        });
    }

    #[test]
    fn streaming_link_loss_stops_the_camera_before_recovery_is_published() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = viva_fake_gige::FakeCamera::builder()
                .bind_ip(Ipv4Addr::LOCALHOST)
                .width(64)
                .height(48)
                .enforce_heartbeat(true)
                .heartbeat_timeout_ms(600)
                .build()
                .await
                .expect("fake");
            let backend = EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let stream = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            fake_camera::first_frame(&stream.ws_url).await;
            fake.unplug();
            wait_state(&backend, |s| {
                matches!(s, ConnectionState::Reconnecting { .. })
            })
            .await;
            fake.commands().reset();
            fake.revoke_control().await;
            fake.plug_in();
            wait_state(&backend, |s| matches!(s, ConnectionState::Connected { .. })).await;
            assert!(backend.acquisition.lock().await.is_none());
            let stops = fake
                .commands()
                .at(GvcpCommand::WriteReg, viva_fake_gige::REG_ACQ_STOP)
                + fake
                    .commands()
                    .at(GvcpCommand::WriteMem, viva_fake_gige::REG_ACQ_STOP);
            assert!(
                stops > 0,
                "the reopened session stopped the existing acquisition"
            );
            assert_eq!(
                fake.commands().at(GvcpCommand::WriteReg, REG_ACQ_START)
                    + fake.commands().at(GvcpCommand::WriteMem, REG_ACQ_START),
                0
            );
            let blocks = fake.blocks_sent();
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(
                fake.blocks_sent(),
                blocks,
                "the camera is stopped, not merely the viewer"
            );
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }

    #[test]
    fn losing_a_read_only_session_never_competes_for_control() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let (events, recorder) = Recorder::events();
            let backend = EmbeddedBackend::new(events);
            backend.connect("127.0.0.1").await.expect("connect");
            fake.revoke_control().await;
            wait_state(&backend, |s| {
                matches!(s, ConnectionState::ControlLost { .. })
            })
            .await;
            fake.commands().reset();
            fake.unplug();
            wait_state(&backend, |s| matches!(s, ConnectionState::Error { .. })).await;
            fake.plug_in();
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(backend.camera.lock().await.is_none());
            assert!(fake.controller().await.is_none());
            assert_eq!(
                fake.commands().total(GvcpCommand::WriteReg)
                    + fake.commands().total(GvcpCommand::WriteMem),
                0
            );
            assert!(recorder.of("device-reconnected").is_empty());
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }

    #[test]
    fn cancelling_an_initial_xml_fetch_cannot_publish_a_late_connection() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = viva_fake_gige::FakeCamera::builder()
                .bind_ip(Ipv4Addr::LOCALHOST)
                .delay_ack(0x1_0000, Duration::from_secs(5))
                .build()
                .await
                .expect("fake");
            let backend = EmbeddedBackend::new(Default::default());
            let opening = backend.clone();
            let task =
                tauri::async_runtime::spawn(async move { opening.connect("127.0.0.1").await });
            tokio::time::timeout(Duration::from_secs(5), async {
                while fake.controller().await.is_none() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("XML is being fetched after claiming");
            backend
                .disconnect()
                .await
                .expect("disconnect while opening");
            assert!(task.await.expect("open task").is_err());
            tokio::time::timeout(Duration::from_secs(5), async {
                while fake.controller().await.is_some() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("cancelled opening released control");
            assert!(backend.camera.lock().await.is_none());
            assert!(backend.model.read().await.xml.is_none());
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Disconnected
            ));
            fake.stop().await;
        });
    }

    #[test]
    fn shutdown_releases_the_old_camera_when_a_connect_is_queued_behind_it() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let transition = backend.lifecycle.transition.lock().await;
            let closing = backend.clone();
            let shutdown = tauri::async_runtime::spawn(async move {
                closing.shutdown().await;
            });
            while !backend
                .lifecycle
                .shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
            {
                tokio::task::yield_now().await;
            }
            let opening = backend.clone();
            let connect =
                tauri::async_runtime::spawn(async move { opening.connect("127.0.0.1").await });
            drop(transition);
            shutdown.await.expect("shutdown");
            assert!(connect.await.expect("connect task").is_err());
            assert!(backend.camera.lock().await.is_none());
            assert!(fake.controller().await.is_none());
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Disconnected
            ));
            fake.stop().await;
        });
    }

    #[test]
    fn read_only_ceiling_preserves_unavailable_and_unreadable_features() {
        for (from, to) in [
            ("RW", "RO"),
            ("RO", "RO"),
            ("WO", "NA"),
            ("NA", "NA"),
            ("NI", "NI"),
        ] {
            let mut state = FeatureState::new(serde_json::Value::Null, from, "Integer");
            cap_read_only(&mut state);
            assert_eq!(state.access_mode, to);
        }
        assert_eq!(parse_mac("00:0C:DF:01:02:03"), Some([0, 12, 223, 1, 2, 3]));
        assert!(parse_mac("00:0C:DF:01:02").is_none());
        assert!(parse_mac("00:0C:DF:01:02:xx").is_none());
    }

    #[test]
    fn idle_loss_recovers_fresh_xml_and_leaves_acquisition_stopped() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let (events, recorder) = Recorder::events();
            let backend = EmbeddedBackend::new(events);
            let first = fake_camera::start().await;
            backend.connect("127.0.0.1").await.expect("connect");
            backend
                .set_feature("BlackLevel", &serde_json::json!(7))
                .await
                .expect("write");
            assert!(
                backend
                    .model
                    .read()
                    .await
                    .graph
                    .as_ref()
                    .unwrap()
                    .nodes_by_name
                    .contains_key("ExposureTime")
            );
            first.stop().await;
            wait_state(&backend, |s| {
                matches!(s, ConnectionState::Reconnecting { .. })
            })
            .await;
            let second = viva_fake_gige::FakeCamera::builder()
                .bind_ip(Ipv4Addr::LOCALHOST)
                .width(64)
                .height(48)
                .profile(viva_fake_gige::XmlProfile::Legacy)
                .model("Replacement XML")
                .build()
                .await
                .expect("replacement");
            wait_state(&backend, |s| matches!(s, ConnectionState::Connected { .. })).await;
            let model = backend.model.read().await;
            assert!(
                model
                    .graph
                    .as_ref()
                    .unwrap()
                    .nodes_by_name
                    .contains_key("ExposureTimeAbs")
            );
            assert!(
                !model
                    .graph
                    .as_ref()
                    .unwrap()
                    .nodes_by_name
                    .contains_key("ExposureTime")
            );
            drop(model);
            assert!(backend.acquisition.lock().await.is_none());
            assert_eq!(
                second.commands().at(GvcpCommand::WriteReg, REG_ACQ_START)
                    + second.commands().at(GvcpCommand::WriteMem, REG_ACQ_START),
                0
            );
            assert_eq!(
                backend
                    .get_feature_state("BlackLevel")
                    .await
                    .expect("value")
                    .value,
                serde_json::json!(0)
            );
            assert_eq!(recorder.of("device-reconnected").len(), 1);
            let stream = backend
                .start_acquisition(Default::default())
                .await
                .expect("explicit Start");
            fake_camera::first_frame(&stream.ws_url).await;
            backend.disconnect().await.expect("disconnect");
            second.stop().await;
        });
    }

    #[test]
    fn a_cached_address_cannot_open_another_mac() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = EmbeddedBackend::new(Default::default());
            let info = viva_genicam::gige::DeviceInfo {
                mac: [2, 0, 0, 0, 0, 1],
                ..viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::LOCALHOST)
            };
            backend.record_scan(vec![info], vec![]).await;
            let id = backend.discover().await[0].id.clone();
            assert!(
                backend
                    .connect(&id)
                    .await
                    .err()
                    .expect("wrong MAC rejected")
                    .contains("Another camera")
            );
            assert!(fake.controller().await.is_none());
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }

    #[test]
    fn recovery_uses_the_mac_when_the_old_address_no_longer_answers() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let mut backend = EmbeddedBackend::new(Default::default());
            backend.reopen_interface = Some(
                viva_genicam::gige::Iface::from_ipv4(Ipv4Addr::LOCALHOST)
                    .expect("loopback")
                    .name()
                    .into(),
            );
            backend.connect("127.0.0.1").await.expect("connect");
            backend
                .with_camera(|c| {
                    let previous = c.gige_info.as_mut().expect("identity");
                    previous.ip = Ipv4Addr::new(127, 0, 0, 2);
                    previous.local_ip = Some(Ipv4Addr::LOCALHOST);
                    Ok(())
                })
                .await
                .expect("old address");
            backend
                .lose_camera(backend.lifecycle.ticket(), "address changed".into())
                .await;
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Connected { .. }
            ));
            let current = backend.camera.lock().await;
            assert_eq!(
                current.as_ref().unwrap().gige_info.as_ref().unwrap().ip,
                Ipv4Addr::LOCALHOST
            );
            drop(current);
            assert!(backend.acquisition.lock().await.is_none());
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }

    #[test]
    fn a_recovery_deadline_rejects_a_different_camera_at_the_old_address() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let mut backend = EmbeddedBackend::new(Default::default());
            backend.reopen_patience = Duration::from_millis(300);
            backend.connect("127.0.0.1").await.expect("connect");
            fake.stop().await;
            let other = viva_fake_gige::FakeCamera::builder()
                .bind_ip(Ipv4Addr::LOCALHOST)
                .mac([2, 0, 0, 0, 0, 1])
                .build()
                .await
                .expect("other camera");
            let started = Instant::now();
            backend
                .lose_camera(backend.lifecycle.ticket(), "test loss".into())
                .await;
            assert!(started.elapsed() < Duration::from_secs(5));
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Error { .. }
            ));
            assert!(other.controller().await.is_none());
            backend.disconnect().await.expect("disconnect");
            other.stop().await;
        });
    }

    #[test]
    fn explicit_disconnect_switch_and_shutdown_cancel_a_pending_recovery() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            for action in ["disconnect", "switch", "shutdown"] {
                let fake = fake_camera::start().await;
                let (events, recorder) = Recorder::events();
                let backend = EmbeddedBackend::new(events);
                backend.connect("127.0.0.1").await.expect("connect");
                fake.unplug();
                let recovering = backend.clone();
                let ticket = backend.lifecycle.ticket();
                let task = tauri::async_runtime::spawn(async move {
                    recovering.lose_camera(ticket, "test loss".into()).await;
                });
                wait_state(&backend, |s| {
                    matches!(s, ConnectionState::Reconnecting { .. })
                })
                .await;
                let started = Instant::now();
                match action {
                    "disconnect" => backend.disconnect().await.expect("disconnect"),
                    "shutdown" => backend.shutdown().await,
                    _ => {
                        assert!(backend.connect("not-a-camera").await.is_err());
                    }
                }
                tokio::time::timeout(Duration::from_secs(5), task)
                    .await
                    .expect("cancelled within deadline")
                    .expect("task");
                assert!(started.elapsed() < Duration::from_secs(5));
                fake.plug_in();
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert!(backend.camera.lock().await.is_none());
                assert!(recorder.of("device-reconnected").is_empty());
                if action == "shutdown" {
                    assert!(
                        backend.connect("127.0.0.1").await.is_err(),
                        "shutdown is terminal"
                    );
                }
                backend.disconnect().await.expect("cleanup");
                fake.stop().await;
            }
        });
    }

    #[test]
    fn control_takeover_is_read_only_and_never_reclaimed_automatically() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = EmbeddedBackend::new(Default::default());
            backend.connect("127.0.0.1").await.expect("connect");
            let stream = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            fake_camera::first_frame(&stream.ws_url).await;
            fake.revoke_control().await;
            wait_state(&backend, |s| {
                matches!(s, ConnectionState::ControlLost { .. })
            })
            .await;
            assert!(backend.acquisition.lock().await.is_none());
            fake.commands().reset();
            assert_eq!(
                backend
                    .get_feature_state("Width")
                    .await
                    .expect("read-only value")
                    .access_mode,
                "RO"
            );
            assert!(
                backend
                    .set_feature("Width", &serde_json::json!(32))
                    .await
                    .is_err()
            );
            assert!(backend.exec_command("AcquisitionStart").await.is_err());
            assert!(backend.start_acquisition(Default::default()).await.is_err());
            assert!(
                backend
                    .set_persistent_ip(
                        Ipv4Addr::LOCALHOST,
                        [255, 0, 0, 0].into(),
                        Ipv4Addr::UNSPECIFIED
                    )
                    .await
                    .is_err()
            );
            backend.stop_acquisition().await.expect("local Stop");
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(fake.controller().await.is_none());
            assert_eq!(
                fake.commands().total(GvcpCommand::WriteReg)
                    + fake.commands().total(GvcpCommand::WriteMem),
                0
            );
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }

    #[test]
    fn normal_stream_end_and_manual_stop_do_not_trigger_recovery() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let (events, recorder) = Recorder::events();
            let backend = EmbeddedBackend::new(events);
            backend.connect("127.0.0.1").await.expect("connect");
            backend
                .set_feature("AcquisitionMode", &serde_json::json!("SingleFrame"))
                .await
                .expect("SingleFrame");
            let stream = backend
                .start_acquisition(Default::default())
                .await
                .expect("start");
            fake_camera::first_frame(&stream.ws_url).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            backend.stop_acquisition().await.expect("stop");
            let ticket = backend.lifecycle.ticket();
            let (_shutdown, shutdown) = watch::channel(false);
            let (frames, _) = watch::channel(Bytes::new());
            let (info, _) = watch::channel(camera_stream_info(64, 48, "Mono8"));
            run_frame_reader(
                Frames::Ended,
                viva_streamer::ws::AppState {
                    frame_tx: frames,
                    native: Default::default(),
                    info_tx: info,
                },
                shutdown,
                StreamInfoTracker::new(64, 48, "Mono8".into()),
                backend.events.clone(),
                Some((backend.clone(), ticket)),
            )
            .await;
            assert_eq!(
                backend.lifecycle.ticket(),
                ticket,
                "normal EOF never schedules recovery"
            );
            assert!(matches!(
                backend.connection.get().await,
                ConnectionState::Connected { .. }
            ));
            assert!(recorder.of("disconnect-reason").is_empty());
            assert!(recorder.of("device-reconnected").is_empty());
            backend.disconnect().await.expect("disconnect");
            fake.stop().await;
        });
    }
}

#[cfg(test)]
#[path = "embedded_monitor_tests.rs"]
mod monitor_tests;

#[cfg(test)]
#[path = "embedded_metadata_tests.rs"]
mod metadata_tests;

#[cfg(test)]
#[path = "embedded_event_tests.rs"]
mod event_tests;

#[cfg(test)]
#[path = "embedded_tool_tests.rs"]
mod tool_tests;
