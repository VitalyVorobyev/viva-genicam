//! Remote backend: a camera served by `viva-service` over Zenoh.
//!
//! Everything the app does in remote mode lives here, behind
//! [`DeviceBackend`]. It consumes the service's `FeatureState` contract
//! (ADR-0010): states are queried on `nodes/{name}/state` and
//! `nodes/bulk/state`, and followed through the `nodes/*/state`
//! publications, so remote mode shows what embedded mode shows — the same
//! `viva-genapi` computation, one hop away (backlog ST-27).

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, OnceCell, watch};
use viva_streamer::meta;
use viva_xml_model::parse_genicam_xml;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, BulkReadRequest,
    DeviceAnnounce, DeviceReportRequest, DeviceReportResponse, DeviceStatus, DeviceXmlResponse,
    FeatureState, ImageMeta, NodeOpResponse, NodeSetRequest, keys,
};
use zenoh::Session;

use crate::state::connection::Connection;
use crate::state::device_state::{
    ApiVersionMismatch, ConnectionState, DeviceInfo, DeviceRegistry, DisconnectReason, StreamerInfo,
};
use crate::state::events::{
    DeviceLost, DeviceReconnected, Events, StudioEvent, ZenohSessionLost, ZenohSessionRestored,
};

use super::{BackendMode, ConnectResult, DeviceBackend, StartOptions};

/// How long a query to the service may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the service may take to write a report. It reads the bootstrap
/// registers and the whole XML from the camera, in 512-byte reads, and the
/// cameras worth reporting on are the slow and troubled ones (Codex review
/// of #174).
const REPORT_TIMEOUT: Duration = Duration::from_secs(120);

/// A device not announced for this long is dropped from the list.
const ANNOUNCE_EXPIRY_SECS: u64 = 6;

const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// The streamer this backend runs for an acquisition: a Zenoh image
/// subscriber feeding the viewer's WebSocket.
struct Streamer {
    shutdown_tx: watch::Sender<bool>,
    tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
}

pub struct RemoteBackend {
    /// This backend's own `Arc`, for the tasks a `&self` method spawns.
    me: Weak<Self>,
    events: Arc<Events>,
    connection: Arc<Connection>,
    session: OnceCell<Arc<Session>>,
    registry: Mutex<DeviceRegistry>,
    /// The connected device's id.
    device: Mutex<Option<String>>,
    /// Subscriptions for the connected device; aborted on disconnect.
    subscriptions: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    streamer: Mutex<Option<Streamer>>,
}

impl RemoteBackend {
    pub fn new(events: Arc<Events>, connection: Arc<Connection>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            events,
            connection,
            session: OnceCell::new(),
            registry: Mutex::new(DeviceRegistry::default()),
            device: Mutex::new(None),
            subscriptions: Mutex::new(Vec::new()),
            streamer: Mutex::new(None),
        })
    }

    /// Open the Zenoh session in the background, then follow announcements.
    pub fn start(self: &Arc<Self>, config: zenoh::Config) {
        let backend = self.clone();
        tauri::async_runtime::spawn(async move {
            match zenoh::open(config).await {
                Ok(session) => {
                    tracing::info!("Zenoh session open, ZID: {}", session.zid());
                    backend.start_with_session(Arc::new(session));
                }
                Err(e) => tracing::error!("Failed to open Zenoh session: {e}"),
            }
        });
    }

    /// Use an open session and follow announcements on it. Tests share one
    /// session between the service and this backend.
    pub fn start_with_session(self: &Arc<Self>, session: Arc<Session>) {
        if self.session.set(session).is_err() {
            tracing::warn!("Zenoh session already set");
            return;
        }
        let backend = self.clone();
        tauri::async_runtime::spawn(async move { backend.run_discovery().await });
        let backend = self.clone();
        tauri::async_runtime::spawn(async move { backend.run_health_monitor().await });
    }

    fn session(&self) -> Result<Arc<Session>, String> {
        self.session
            .get()
            .cloned()
            .ok_or_else(|| "Zenoh session not initialized".to_string())
    }

    async fn device_id(&self) -> Result<String, String> {
        self.device
            .lock()
            .await
            .clone()
            .ok_or_else(|| "Not connected to a device".to_string())
    }

    // ── Discovery ───────────────────────────────────────────────────────────

    async fn run_discovery(self: Arc<Self>) {
        let Ok(session) = self.session() else {
            return;
        };
        let sub = match session.declare_subscriber(keys::ANNOUNCE_ALL).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Discovery subscriber error: {e}");
                return;
            }
        };
        tracing::info!("Discovery loop started");
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                result = sub.recv_async() => {
                    let Ok(sample) = result else { break };
                    match serde_json::from_slice::<DeviceAnnounce>(&sample.payload().to_bytes()) {
                        Ok(announce) => self.on_announce(announce).await,
                        Err(e) => tracing::warn!("Failed to parse announce: {e}"),
                    }
                }
                _ = ticker.tick() => {
                    let expired = self.registry.lock().await.expire_old(ANNOUNCE_EXPIRY_SECS);
                    for id in expired {
                        self.events.send(&DeviceLost { device_id: id });
                    }
                }
            }
        }
    }

    async fn on_announce(&self, announce: DeviceAnnounce) {
        let api_version = announce.api_version;
        let info = DeviceInfo::from_announce(announce, "zenoh");
        let is_new = {
            let mut registry = self.registry.lock().await;
            let is_new = !registry.contains(&info.id);
            registry.update(info.clone());
            is_new
        };
        if is_new {
            tracing::info!(device_id = %info.id, "New device discovered");
        }
        // Always emit, so a UI listener registered after the first announce
        // still sees the device; the UI deduplicates by id.
        self.events.send(&info);
        if is_new && api_version != Some(viva_zenoh_api::API_VERSION) {
            self.events.send(&ApiVersionMismatch {
                device_id: info.id.clone(),
                device_version: api_version,
                app_version: viva_zenoh_api::API_VERSION,
            });
        }
    }

    /// Emit `zenoh-session-lost` / `-restored` as a liveliness token stops
    /// or resumes being declarable.
    async fn run_health_monitor(self: Arc<Self>) {
        let Ok(session) = self.session() else {
            return;
        };
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        let mut was_healthy = true;
        loop {
            interval.tick().await;
            let healthy = session
                .liveliness()
                .declare_token("viva-studio/health")
                .await
                .is_ok();
            if !healthy && was_healthy {
                tracing::warn!("Zenoh session health check failed");
                self.events.send(&ZenohSessionLost {
                    message: "Zenoh session lost".to_string(),
                });
            } else if healthy && !was_healthy {
                tracing::info!("Zenoh session recovered");
                self.events.send(&ZenohSessionRestored {
                    message: "Zenoh session restored".to_string(),
                });
            }
            was_healthy = healthy;
        }
    }

    // ── Per-device subscriptions ────────────────────────────────────────────

    async fn subscribe(self: &Arc<Self>, session: &Arc<Session>, device_id: &str) {
        self.unsubscribe().await;
        let tasks = vec![
            self.clone()
                .follow_states(session.clone(), device_id.to_string()),
            self.clone()
                .follow_status(session.clone(), device_id.to_string()),
            self.clone()
                .follow::<AcquisitionStatus>(session.clone(), keys::acquisition_status(device_id)),
            self.clone()
                .follow::<ImageMeta>(session.clone(), keys::image_meta(device_id)),
        ];
        self.subscriptions.lock().await.extend(tasks);
    }

    async fn unsubscribe(&self) {
        for task in self.subscriptions.lock().await.drain(..) {
            task.abort();
        }
    }

    /// Re-emit every `FeatureState` the service publishes, in the shape
    /// embedded mode sends.
    fn follow_states(
        self: Arc<Self>,
        session: Arc<Session>,
        device_id: String,
    ) -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            let key = keys::node_state_wildcard(&device_id);
            let sub = match session.declare_subscriber(&key).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Feature state subscriber error: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let Some(name) = keys::extract_node_name(sample.key_expr().as_str()) else {
                    continue;
                };
                if let Ok(state) =
                    serde_json::from_slice::<FeatureState>(&sample.payload().to_bytes())
                {
                    self.events.node_state(name, &state);
                }
            }
        })
    }

    /// Forward a topic's payloads to the UI as their event.
    fn follow<T: StudioEvent + DeserializeOwned + Send + 'static>(
        self: Arc<Self>,
        session: Arc<Session>,
        key: String,
    ) -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            let sub = match session.declare_subscriber(&key).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("{} subscriber error: {e}", T::NAME);
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                if let Ok(payload) = serde_json::from_slice::<T>(&sample.payload().to_bytes()) {
                    self.events.send(&payload);
                }
            }
        })
    }

    /// When the service reports the device gone, drop it and try to get it
    /// back.
    fn follow_status(
        self: Arc<Self>,
        session: Arc<Session>,
        device_id: String,
    ) -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            let sub = match session.declare_subscriber(keys::status(&device_id)).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Status subscriber error: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let Ok(status) =
                    serde_json::from_slice::<DeviceStatus>(&sample.payload().to_bytes())
                else {
                    continue;
                };
                if status.connected {
                    continue;
                }
                let message = status
                    .error
                    .unwrap_or_else(|| "Device disconnected".to_string());
                // This task is one of the subscriptions the reconnect loop
                // replaces, so the loop runs on its own task.
                let backend = self.clone();
                tauri::async_runtime::spawn(async move {
                    backend.lose_device(device_id, message).await;
                });
                return;
            }
        })
    }

    async fn lose_device(self: Arc<Self>, device_id: String, message: String) {
        self.unsubscribe().await;
        self.stop_streamer().await;
        self.connection
            .set(ConnectionState::Reconnecting {
                device_id: device_id.clone(),
                attempt: 0,
                max_attempts: MAX_RECONNECT_ATTEMPTS,
                reason: message.clone(),
            })
            .await;
        self.events.send(&DisconnectReason {
            message,
            device_id: device_id.clone(),
        });
        self.reconnect(device_id).await;
    }

    /// Wait for the device to be announced again, then resubscribe.
    async fn reconnect(self: Arc<Self>, device_id: String) {
        let backoff_secs = [1, 2, 4, 8, 15];
        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            let delay = backoff_secs
                .get((attempt - 1) as usize)
                .copied()
                .unwrap_or(15);
            tokio::time::sleep(Duration::from_secs(delay)).await;

            // The user disconnected or connected elsewhere meanwhile.
            if !matches!(
                self.connection.get().await,
                ConnectionState::Reconnecting { .. }
            ) {
                return;
            }
            let reason = if !self.registry.lock().await.contains(&device_id) {
                "Waiting for device to reappear".to_string()
            } else {
                match self.attach(&device_id).await {
                    Ok(result) => {
                        let node_count = parse_genicam_xml(&result.xml)
                            .map(|g| g.nodes_by_name.len())
                            .unwrap_or_default();
                        self.connection
                            .set(ConnectionState::Connected {
                                device_id: device_id.clone(),
                                device_name: result.device_name,
                                model: result.model,
                            })
                            .await;
                        self.events.send(&DeviceReconnected {
                            device_id: device_id.clone(),
                            node_count,
                        });
                        tracing::info!("Reconnected to {device_id}");
                        return;
                    }
                    Err(e) => format!("Reconnect failed: {e}"),
                }
            };
            tracing::info!("Reconnect attempt {attempt}/{MAX_RECONNECT_ATTEMPTS}: {reason}");
            self.connection
                .set(ConnectionState::Reconnecting {
                    device_id: device_id.clone(),
                    attempt,
                    max_attempts: MAX_RECONNECT_ATTEMPTS,
                    reason,
                })
                .await;
        }
        tracing::warn!(
            "Reconnection to {device_id} failed after {MAX_RECONNECT_ATTEMPTS} attempts"
        );
        self.connection
            .set(ConnectionState::Error {
                message: format!(
                    "Device '{device_id}' lost. Reconnection failed after {MAX_RECONNECT_ATTEMPTS} attempts."
                ),
            })
            .await;
    }

    /// Fetch the device's XML and follow its topics; the shared half of
    /// connecting and reconnecting.
    async fn attach(self: &Arc<Self>, device_id: &str) -> Result<ConnectResult, String> {
        let session = self.session()?;
        let response: DeviceXmlResponse = query(&session, &keys::xml(device_id), None).await?;
        self.subscribe(&session, device_id).await;
        *self.device.lock().await = Some(device_id.to_string());
        let (device_name, model) = {
            let registry = self.registry.lock().await;
            (
                registry
                    .get_name(device_id)
                    .unwrap_or_else(|| device_id.to_string()),
                registry.get_model(device_id).unwrap_or_default(),
            )
        };
        Ok(ConnectResult {
            xml: response.xml,
            device_name,
            model,
        })
    }

    // ── Streaming ───────────────────────────────────────────────────────────

    async fn stop_streamer(&self) {
        if let Some(streamer) = self.streamer.lock().await.take() {
            let _ = streamer.shutdown_tx.send(true);
            for task in streamer.tasks {
                task.abort();
                let _ = task.await;
            }
        }
    }

    /// The image size to announce before the first frame says otherwise.
    async fn image_size(&self) -> (u32, u32) {
        let names = ["Width".to_string(), "Height".to_string()];
        let states = self.bulk_feature_state(&names).await.unwrap_or_default();
        let dim = |name: &str, default: u32| {
            states
                .get(name)
                .and_then(|s| s.value.as_u64())
                .map(|v| v as u32)
                .unwrap_or(default)
        };
        (dim("Width", 640), dim("Height", 480))
    }

    async fn send_acquisition(&self, request: AcquisitionControlRequest) -> Result<(), String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        let payload = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        let resp: NodeOpResponse = query(
            &session,
            &keys::acquisition_control(&device_id),
            Some(payload),
        )
        .await?;
        if resp.ok {
            Ok(())
        } else {
            Err(resp
                .error
                .unwrap_or_else(|| "Acquisition command failed".to_string()))
        }
    }
}

#[async_trait]
impl DeviceBackend for RemoteBackend {
    fn mode(&self) -> BackendMode {
        BackendMode::Remote
    }

    async fn discover(&self) -> Vec<DeviceInfo> {
        self.registry.lock().await.list()
    }

    async fn connect(&self, device_id: &str) -> Result<ConnectResult, String> {
        self.disconnect().await?;
        // `attach` hands an `Arc` of this backend to its subscriptions.
        let this = self.self_arc()?;
        this.attach(device_id).await
    }

    async fn disconnect(&self) -> Result<(), String> {
        self.unsubscribe().await;
        // Stop what this app started: a camera left streaming would go on
        // sending frames nobody watches.
        if self.streamer.lock().await.is_some() {
            if let Err(e) = self
                .send_acquisition(AcquisitionControlRequest::new(AcquisitionCommand::Stop))
                .await
            {
                tracing::debug!("AcquisitionStop on disconnect: {e}");
            }
            self.stop_streamer().await;
        }
        *self.device.lock().await = None;
        Ok(())
    }

    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        query(&session, &keys::node_introspect(&device_id, name), None).await
    }

    async fn bulk_feature_state(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, FeatureState>, String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        let payload = serde_json::to_vec(&BulkReadRequest {
            names: names.to_vec(),
        })
        .map_err(|e| e.to_string())?;
        query(&session, &keys::nodes_bulk_state(&device_id), Some(payload)).await
    }

    async fn set_feature(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        let payload = serde_json::to_vec(&NodeSetRequest {
            value: value.clone(),
        })
        .map_err(|e| e.to_string())?;
        let resp: NodeOpResponse =
            query(&session, &keys::node_set(&device_id, name), Some(payload)).await?;
        if resp.ok {
            Ok(resp.invalidated)
        } else {
            Err(resp.error.unwrap_or_else(|| "Write failed".to_string()))
        }
    }

    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        let resp: NodeOpResponse =
            query(&session, &keys::node_execute(&device_id, name), None).await?;
        if resp.ok {
            Ok(resp.invalidated)
        } else {
            Err(resp
                .error
                .unwrap_or_else(|| format!("Executing '{name}' failed")))
        }
    }

    async fn start_acquisition(&self, options: StartOptions) -> Result<StreamerInfo, String> {
        let session = self.session()?;
        let device_id = self.device_id().await?;
        let (width, height) = self.image_size().await;
        self.stop_streamer().await;

        let image_key = keys::image(&device_id);
        let meta_key = meta::derive_meta_key(&image_key);
        let (frame_tx, _) = watch::channel(Bytes::new());
        let info_tx = watch::channel(viva_streamer::ws::StreamInfo::from_image_meta(
            &meta::default_image_meta(width, height),
        ))
        .0;
        let shared_meta = Arc::new(tokio::sync::RwLock::new(meta::default_image_meta(
            width, height,
        )));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("Failed to bind WS listener: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("Failed to get local address: {e}"))?
            .port();
        let ws_url = format!("ws://127.0.0.1:{port}/ws");
        tracing::info!("Remote streamer WS on {ws_url}");

        // Subscribe before asking the service to start, so the first frames
        // have a subscriber to reach.
        let source_config = viva_streamer::zenoh_source::ZenohSourceConfig {
            key_expr: image_key,
            meta_key,
            fps_limit: Some(15),
        };
        let source = tauri::async_runtime::spawn({
            let frame_tx = frame_tx.clone();
            let info_tx = info_tx.clone();
            let shutdown_rx = shutdown_rx.clone();
            async move {
                if let Err(e) = viva_streamer::zenoh_source::run_with_session(
                    session,
                    source_config,
                    shared_meta,
                    frame_tx,
                    info_tx,
                    shutdown_rx,
                )
                .await
                {
                    tracing::error!("Zenoh source task error: {e}");
                }
            }
        });
        let server = tauri::async_runtime::spawn({
            let state = viva_streamer::ws::AppState { frame_tx, info_tx };
            async move {
                if let Err(e) = viva_streamer::ws::run_server_with_listener(
                    listener,
                    "/ws".to_string(),
                    state,
                    shutdown_rx,
                )
                .await
                {
                    tracing::error!("WS server task error: {e}");
                }
            }
        });
        *self.streamer.lock().await = Some(Streamer {
            shutdown_tx,
            tasks: vec![source, server],
        });

        // Let the subscription's interest reach the service's peer first.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = AcquisitionControlRequest {
            packet_size: options.packet_size,
            ..AcquisitionControlRequest::new(AcquisitionCommand::Start)
        };
        if let Err(e) = self.send_acquisition(start).await {
            self.stop_streamer().await;
            return Err(e);
        }
        Ok(StreamerInfo {
            ws_url,
            width,
            height,
        })
    }

    async fn stop_acquisition(&self) -> Result<(), String> {
        // Best-effort: the local streamer goes either way.
        let result = self
            .send_acquisition(AcquisitionControlRequest::new(AcquisitionCommand::Stop))
            .await;
        self.stop_streamer().await;
        result
    }

    async fn shutdown(&self) {
        if let Err(e) = self.disconnect().await {
            tracing::debug!("disconnect at exit: {e}");
        }
    }

    /// The camera may be out of this host's reach, so the service writes the
    /// report where it runs; with no device connected, Studio reports on its
    /// own host.
    async fn report(&self, include_xml: bool) -> Result<String, String> {
        let header = format!(
            "Viva Studio {} in remote mode; the service wrote the report below.\n\n",
            env!("CARGO_PKG_VERSION")
        );
        match self.device_id().await {
            Ok(device_id) => {
                let session = self.session()?;
                let payload = serde_json::to_vec(&DeviceReportRequest { include_xml })
                    .map_err(|e| e.to_string())?;
                let key = keys::report(&device_id);
                let response: DeviceReportResponse =
                    query_within(&session, &key, Some(payload), REPORT_TIMEOUT).await?;
                Ok(header + &response.report)
            }
            Err(_) => {
                use viva_genicam::report::{ReportRequest, ReportTarget, build_report};
                let request = ReportRequest {
                    notes: vec!["mode:        remote, no device connected".to_string()],
                    include_xml,
                    ..ReportRequest::new(super::embedded::studio_tool(), ReportTarget::None)
                };
                Ok(build_report(&request).await)
            }
        }
    }
}

impl RemoteBackend {
    /// The `Arc` this backend lives in. [`RemoteBackend::new`] only hands out
    /// `Arc`s, and the trait methods take `&self`, so the subscriptions that
    /// need to outlive a call get it from here.
    fn self_arc(&self) -> Result<Arc<Self>, String> {
        self.me
            .upgrade()
            .ok_or_else(|| "The remote backend is shutting down".to_string())
    }
}

/// Ask the service and decode the first successful reply.
///
/// `nodes/bulk/state` also matches the per-node `nodes/*/state` queryable,
/// which answers a node named `bulk` with an error; error replies are
/// therefore skipped while any other reply may still come.
async fn query<T: DeserializeOwned>(
    session: &Session,
    key: &str,
    payload: Option<Vec<u8>>,
) -> Result<T, String> {
    query_within(session, key, payload, QUERY_TIMEOUT).await
}

/// [`query`], waiting up to `timeout` for the reply.
async fn query_within<T: DeserializeOwned>(
    session: &Session,
    key: &str,
    payload: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<T, String> {
    let mut get = session.get(key).timeout(timeout);
    if let Some(payload) = payload {
        get = get.payload(payload);
    }
    let replies = get.await.map_err(|e| format!("Zenoh error: {e}"))?;
    let mut refused = None;
    while let Ok(reply) = replies.recv_async().await {
        match reply.result() {
            Ok(sample) => {
                return serde_json::from_slice(&sample.payload().to_bytes())
                    .map_err(|e| format!("Invalid reply on {key}: {e}"));
            }
            Err(e) => {
                refused = Some(String::from_utf8_lossy(&e.payload().to_bytes()).into_owned());
            }
        }
    }
    Err(refused.unwrap_or_else(|| format!("No reply for {key} (timeout)")))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::watch;
    use viva_genicam::gige::nic::Iface;
    use viva_service::device::DeviceHandle;
    use viva_service::serve::{derive_device_id, publish_announce, serve_device};

    use super::RemoteBackend;
    use crate::backend::DeviceBackend;
    use crate::backend::embedded::fake_camera;
    use crate::state::connection::Connection;
    use crate::state::events::Recorder;

    /// Remote mode end to end: `viva-service` serves the fake camera in
    /// process, and the backend discovers it, reads `viva-genapi`'s feature
    /// state through the service, writes, streams and lets go — the path ST-27
    /// rebuilt. Before it, remote mode answered a feature query from a cache
    /// the bulk read never filled, with `kind: "Unknown"` and no enumeration
    /// entries, and every method here returned an error.
    #[test]
    fn remote_mode_drives_the_camera_through_the_service() {
        let _port = fake_camera::lock();
        tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());

            // The service, as its binary runs it.
            let devices = viva_genicam::gige::discover_all(Duration::from_secs(2))
                .await
                .expect("discovery");
            let info = devices
                .iter()
                .find(|d| d.ip == Ipv4Addr::LOCALHOST)
                .expect("the fake camera on loopback")
                .clone();
            let iface = Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback");
            let handle = Arc::new(DeviceHandle::connect(&info, Some(iface)).await.unwrap());
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let service = serve_device(session.clone(), handle, shutdown_rx).await;

            // The app.
            let (events, recorder) = Recorder::events();
            let backend = RemoteBackend::new(events.clone(), Arc::new(Connection::new(events)));
            backend.start_with_session(session.clone());
            let device_id = derive_device_id(&info);
            let discovered = async {
                loop {
                    publish_announce(&session, &info).await;
                    if backend.discover().await.iter().any(|d| d.id == device_id) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), discovered)
                .await
                .expect("the announce reaches the device list");

            let connected = backend.connect(&device_id).await.expect("connect");
            assert!(connected.xml.contains("PixelFormat"));

            // The state is `viva-genapi`'s, through the service.
            let pixel_format = backend.get_feature_state("PixelFormat").await.unwrap();
            assert_eq!(pixel_format.kind, "Enumeration");
            let mut entries = pixel_format.enum_available.expect("enum entries");
            entries.sort();
            assert_eq!(entries, ["Mono16", "Mono8"]);

            // A write names what it invalidated, and the lock shows.
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

            // `nodes/bulk/state` also reaches the per-node queryable, which
            // refuses a node named "bulk"; the bulk answer must still win.
            let names = ["Width".to_string(), "Height".to_string()];
            let bulk = backend.bulk_feature_state(&names).await.unwrap();
            assert_eq!(bulk["Width"].value, serde_json::json!(64));
            assert_eq!(bulk["Height"].value, serde_json::json!(48));

            // The service publishes states as they change; they reach the UI
            // in the shape embedded mode sends.
            let published = async {
                loop {
                    let states = recorder.of("feature-state-changed");
                    if states
                        .iter()
                        .any(|e| e["name"] == "ExposureAuto" && e["state"].is_object())
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(5), published)
                .await
                .expect("the published state of ExposureAuto reaches the UI");

            // A packet size the user set reaches the camera through the
            // service (ST-22).
            let options = crate::backend::StartOptions {
                packet_size: Some(1000),
            };
            let streamer = backend.start_acquisition(options).await.expect("start");
            let scps = {
                use viva_fake_gige::registers::{SCP_PACKET_SIZE, STREAM_CHANNEL_BASE};
                let bytes = fake.peek(STREAM_CHANNEL_BASE + SCP_PACKET_SIZE, 4).await;
                u32::from_be_bytes(bytes.try_into().unwrap()) & 0xFFFF
            };
            assert_eq!(scps, 1000);
            fake_camera::first_frame(&streamer.ws_url).await;
            backend.stop_acquisition().await.expect("stop");

            // The report comes from the service, which can reach the camera.
            let report = backend.report(false).await.expect("report");
            assert!(report.contains("in remote mode"), "{report}");
            assert!(report.contains("tool:        viva-service"), "{report}");
            assert!(report.contains("selected: 127.0.0.1"), "{report}");

            // The service stops; the backend is still connected.
            let _ = shutdown_tx.send(true);
            for task in service {
                let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
            }

            // A report takes as long as the camera needs — discovery, the
            // bootstrap registers, the whole XML in 512-byte reads — and a
            // slow camera needs more than the 5 s other queries get: the
            // backend gave up while the service was still writing (Codex
            // review of #174). A stand-in service answers after 6 s.
            let slow = session
                .declare_queryable(viva_zenoh_api::keys::report(&device_id))
                .await
                .unwrap();
            let stand_in = tauri::async_runtime::spawn(async move {
                let query = slow.recv_async().await.unwrap();
                tokio::time::sleep(Duration::from_secs(6)).await;
                let response = viva_zenoh_api::DeviceReportResponse {
                    report: "written slowly".to_string(),
                };
                let payload = serde_json::to_vec(&response).unwrap();
                query
                    .reply(query.key_expr().clone(), payload)
                    .await
                    .unwrap();
            });
            let report = backend.report(false).await.expect("a slow report");
            assert!(report.contains("written slowly"), "{report}");
            stand_in.await.unwrap();

            backend.disconnect().await.expect("disconnect");
            assert!(backend.get_feature_state("Width").await.is_err());
        });
    }
}
