use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;

use crate::commands::xml_model::{ModelSummary, ParseXmlResponse};
use crate::error::HumanizeExt;
use crate::state::ModelState;
use crate::state::device_state::{
    ApiVersionMismatch, ConnectionState, DeviceInfo, DisconnectReason, NodeValueEntry, ZenohState,
};
use viva_xml_model::parse_genicam_xml;
use viva_zenoh_api::{DeviceAnnounce, DeviceXmlResponse, ImageMeta};

// ── API version compatibility ─────────────────────────────────────────────────

/// Outcome of comparing a service-announced API version against the app's expected version.
#[derive(Debug, PartialEq)]
pub enum ApiVersionStatus {
    /// Versions match exactly — service is compatible.
    Compatible,
    /// Service did not include `api_version` — old service, warn but allow.
    Missing,
    /// Service announced a different version — warn user.
    Incompatible(u32),
}

/// Compare the announced `api_version` against the app's `expected` version.
fn check_api_version(announced: Option<u32>, expected: u32) -> ApiVersionStatus {
    match announced {
        None => ApiVersionStatus::Missing,
        Some(v) if v == expected => ApiVersionStatus::Compatible,
        Some(v) => ApiVersionStatus::Incompatible(v),
    }
}

// ── IPC Commands ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_discovered_devices(
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
) -> Result<Vec<DeviceInfo>, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        return Ok(backend.discover().await);
    }
    Ok(zenoh.registry.lock().await.list())
}

#[tauri::command]
pub async fn get_connection_state(
    zenoh: State<'_, Arc<ZenohState>>,
) -> Result<ConnectionState, String> {
    Ok(zenoh.connection.lock().await.clone())
}

#[tauri::command]
pub async fn connect_device(
    device_id: String,
    zenoh: State<'_, Arc<ZenohState>>,
    model: State<'_, RwLock<ModelState>>,
    backend: State<'_, crate::backend::BackendState>,
    app: AppHandle,
) -> Result<ParseXmlResponse, String> {
    // ── Embedded mode path ───────────────────────────────────────────────────
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        *zenoh.connection.lock().await = ConnectionState::Connecting {
            device_id: device_id.clone(),
        };
        emit_connection_state(&app, &zenoh).await;

        let result = match backend.connect(&device_id).await {
            Ok(r) => r,
            Err(e) => {
                *zenoh.connection.lock().await = ConnectionState::Error { message: e.clone() };
                emit_connection_state(&app, &zenoh).await;
                return Err(e);
            }
        };

        let graph = parse_genicam_xml(&result.xml).map_err(|e| e.to_string())?;
        let summary = ModelSummary {
            node_count: graph.nodes_by_name.len(),
            category_count: graph.categories.len(),
            root_category: graph.root_category.clone(),
        };
        let response = ParseXmlResponse {
            graph: graph.clone(),
            xml: result.xml.clone(),
            diags: vec![],
            summary,
        };
        model
            .write()
            .await
            .update(result.xml, graph, vec![], Some(device_id.clone()));

        *zenoh.connection.lock().await = ConnectionState::Connected {
            device_id,
            device_name: result.device_name,
            model: result.model,
        };
        emit_connection_state(&app, &zenoh).await;

        return Ok(response);
    }

    // ── Remote mode path (Zenoh) ─────────────────────────────────────────────
    let session = zenoh.get_session().await.humanize()?;

    // Signal connecting
    *zenoh.connection.lock().await = ConnectionState::Connecting {
        device_id: device_id.clone(),
    };
    emit_connection_state(&app, &zenoh).await;

    // Fetch GenICam XML from the device service
    let xml = match fetch_device_xml(&session, &device_id).await.humanize() {
        Ok(xml) => xml,
        Err(e) => {
            *zenoh.connection.lock().await = ConnectionState::Error { message: e.clone() };
            emit_connection_state(&app, &zenoh).await;
            return Err(e);
        }
    };

    // Parse the XML into a UiGraph
    let graph = parse_genicam_xml(&xml).map_err(|e| e.to_string())?;

    let summary = ModelSummary {
        node_count: graph.nodes_by_name.len(),
        category_count: graph.categories.len(),
        root_category: graph.root_category.clone(),
    };

    let response = ParseXmlResponse {
        graph: graph.clone(),
        xml: xml.clone(),
        diags: vec![],
        summary,
    };

    // Persist in model state so get_current_model() still works
    model
        .write()
        .await
        .update(xml, graph, vec![], Some(device_id.clone()));

    // Resolve display name from registry
    let (device_name, device_model) = {
        let registry = zenoh.registry.lock().await;
        (
            registry
                .get_name(&device_id)
                .unwrap_or_else(|| device_id.clone()),
            registry.get_model(&device_id).unwrap_or_default(),
        )
    };

    // Abort any stale subscription tasks and clear node cache
    abort_sub_tasks(&zenoh).await;
    zenoh.node_cache.write().await.clear();

    // Start per-connection subscriber tasks
    let z = zenoh.inner().clone();
    let tasks = vec![
        spawn_node_value_sub(session.clone(), device_id.clone(), z.clone(), app.clone()),
        spawn_status_sub(session.clone(), device_id.clone(), z.clone(), app.clone()),
        spawn_acq_status_sub(session.clone(), device_id.clone(), z.clone(), app.clone()),
        spawn_image_meta_sub(session.clone(), device_id.clone(), z.clone(), app.clone()),
    ];
    zenoh.sub_tasks.lock().await.extend(tasks);

    // Mark connected
    *zenoh.connection.lock().await = ConnectionState::Connected {
        device_id,
        device_name,
        model: device_model,
    };
    emit_connection_state(&app, &zenoh).await;

    Ok(response)
}

#[tauri::command]
pub async fn disconnect_device(
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
    app: AppHandle,
) -> Result<(), String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        let device_id = match zenoh.connection.lock().await.clone() {
            ConnectionState::Connected { device_id, .. } => device_id,
            _ => String::new(),
        };
        backend.disconnect(&device_id).await.ok();
        *zenoh.connection.lock().await = ConnectionState::Disconnected;
        emit_connection_state(&app, &zenoh).await;
        return Ok(());
    }
    abort_sub_tasks(&zenoh).await;
    stop_acquisition_child(&zenoh).await;
    zenoh.node_cache.write().await.clear();
    *zenoh.connection.lock().await = ConnectionState::Disconnected;
    emit_connection_state(&app, &zenoh).await;
    Ok(())
}

// ── Background task init (called from main.rs setup) ─────────────────────────

/// Start the global device-discovery subscriber.  Runs for the app lifetime.
pub fn start_discovery_task(state: Arc<ZenohState>, app: AppHandle) {
    let state_clone = state.clone();
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        run_discovery_loop(state_clone, app_clone).await;
    });
    tauri::async_runtime::spawn(async move {
        run_session_health_monitor(state, app).await;
    });
}

/// Periodically checks Zenoh session health via a liveliness token.
/// Emits `zenoh-session-lost` event if the session becomes unresponsive.
async fn run_session_health_monitor(zenoh: Arc<ZenohState>, app: AppHandle) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    let mut was_healthy = true;

    loop {
        interval.tick().await;

        let session = match zenoh.session.lock().await.clone() {
            Some(s) => s,
            None => continue,
        };

        // Try a lightweight liveliness declaration as health check
        let healthy = session
            .liveliness()
            .declare_token("viva-studio/health")
            .await
            .is_ok();

        if !healthy && was_healthy {
            tracing::warn!("Zenoh session health check failed");
            let _ = app.emit(
                "zenoh-session-lost",
                serde_json::json!({ "message": "Zenoh session lost" }),
            );
        } else if healthy && !was_healthy {
            tracing::info!("Zenoh session recovered");
            let _ = app.emit(
                "zenoh-session-restored",
                serde_json::json!({ "message": "Zenoh session restored" }),
            );
        }

        was_healthy = healthy;
    }
}

async fn run_discovery_loop(zenoh: Arc<ZenohState>, app: AppHandle) {
    let session = match zenoh.get_session().await {
        Ok(s) => s,
        Err(_) => return, // Zenoh not initialized — skip
    };

    let sub = match session
        .declare_subscriber(viva_zenoh_api::keys::ANNOUNCE_ALL)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Discovery subscriber error: {e}");
            return;
        }
    };
    tracing::info!("Discovery loop started");

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        tokio::select! {
            result = sub.recv_async() => {
                let sample = match result {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let bytes = sample.payload().to_bytes();
                let announce = match serde_json::from_slice::<DeviceAnnounce>(&bytes) {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!("Failed to parse announce: {e}");
                        continue;
                    }
                };
                {
                    // Check API version compatibility on first discovery of each device.
                    let api_version = announce.api_version;
                    let version_status =
                        check_api_version(api_version, viva_zenoh_api::API_VERSION);

                    let info = DeviceInfo::from_announce(announce, "zenoh");
                    let is_new = {
                        let mut registry = zenoh.registry.lock().await;
                        let is_new = !registry.contains(&info.id);
                        registry.update(info.clone());
                        is_new
                    };
                    if is_new {
                        tracing::info!(device_id = %info.id, "New device discovered");
                    }
                    // Always emit so the UI picks up the device even if its
                    // listener registered after the first announce.
                    // The UI deduplicates by device ID.
                    let _ = app.emit("device-discovered", &info);
                    if is_new && version_status != ApiVersionStatus::Compatible {
                        let _ = app.emit(
                            "api-version-mismatch",
                            ApiVersionMismatch {
                                device_id: info.id.clone(),
                                device_version: api_version,
                                app_version: viva_zenoh_api::API_VERSION,
                            },
                        );
                    }
                }
            }
            _ = ticker.tick() => {
                let expired = zenoh.registry.lock().await.expire_old(6);
                for id in expired {
                    let _ = app.emit("device-lost", serde_json::json!({ "device_id": id }));
                }
            }
        }
    }
}

// ── Subscription task spawners ───────────────────────────────────────────────

fn spawn_node_value_sub(
    session: Arc<zenoh::Session>,
    device_id: String,
    zenoh: Arc<ZenohState>,
    app: AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let key = viva_zenoh_api::keys::node_value_wildcard(&device_id);
        let sub = match session.declare_subscriber(&key).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Node value subscriber error: {e}");
                return;
            }
        };
        while let Ok(sample) = sub.recv_async().await {
            let bytes = sample.payload().to_bytes();
            if let Ok(update) = serde_json::from_slice::<viva_zenoh_api::NodeValueUpdate>(&bytes) {
                let key_str = sample.key_expr().as_str();
                if let Some(node_name) = viva_zenoh_api::keys::extract_node_name(key_str) {
                    let entry = NodeValueEntry {
                        value: update.value.clone(),
                        access_mode: update.access_mode.clone(),
                        min: update.min,
                        max: update.max,
                        inc: update.inc,
                    };
                    zenoh
                        .node_cache
                        .write()
                        .await
                        .insert(node_name.to_string(), entry);
                    let _ = app.emit(
                        "node-value-changed",
                        serde_json::json!({
                            "node_name": node_name,
                            "value": update.value,
                            "access_mode": update.access_mode,
                            "min": update.min,
                            "max": update.max,
                            "inc": update.inc,
                        }),
                    );
                }
            }
        }
    })
}

fn spawn_status_sub(
    session: Arc<zenoh::Session>,
    device_id: String,
    zenoh: Arc<ZenohState>,
    app: AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let key = viva_zenoh_api::keys::status(&device_id);
        let sub = match session.declare_subscriber(&key).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Status subscriber error: {e}");
                return;
            }
        };
        while let Ok(sample) = sub.recv_async().await {
            let bytes = sample.payload().to_bytes();
            if let Ok(status) = serde_json::from_slice::<viva_zenoh_api::DeviceStatus>(&bytes)
                && !status.connected
            {
                let msg = status
                    .error
                    .unwrap_or_else(|| "Device disconnected".to_string());
                do_emergency_disconnect(&zenoh, &app, device_id.clone(), msg).await;
                // Start auto-reconnection
                spawn_reconnect_task(zenoh.clone(), app.clone(), device_id.clone());
                break;
            }
        }
    })
}

fn spawn_acq_status_sub(
    session: Arc<zenoh::Session>,
    device_id: String,
    zenoh: Arc<ZenohState>,
    app: AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let key = viva_zenoh_api::keys::acquisition_status(&device_id);
        let sub = match session.declare_subscriber(&key).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Acquisition status subscriber error: {e}");
                return;
            }
        };
        while let Ok(sample) = sub.recv_async().await {
            let bytes = sample.payload().to_bytes();
            if let Ok(status) = serde_json::from_slice::<viva_zenoh_api::AcquisitionStatus>(&bytes)
            {
                zenoh.acquisition.lock().await.status = status.clone();
                let _ = app.emit("acquisition-status", status);
            }
        }
    })
}

fn spawn_image_meta_sub(
    session: Arc<zenoh::Session>,
    device_id: String,
    zenoh: Arc<ZenohState>,
    app: AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let key = viva_zenoh_api::keys::image_meta(&device_id);
        let sub = match session.declare_subscriber(&key).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Image meta subscriber error: {e}");
                return;
            }
        };
        while let Ok(sample) = sub.recv_async().await {
            let bytes = sample.payload().to_bytes();
            if let Ok(meta) = serde_json::from_slice::<ImageMeta>(&bytes) {
                zenoh.acquisition.lock().await.image_meta = Some(meta.clone());
                let _ = app.emit("image-meta-changed", meta);
            }
        }
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn emit_connection_state(app: &AppHandle, zenoh: &ZenohState) {
    let state = zenoh.connection.lock().await.clone();
    let _ = app.emit("connection-state-changed", state);
}

async fn abort_sub_tasks(zenoh: &ZenohState) {
    let mut tasks = zenoh.sub_tasks.lock().await;
    for t in tasks.drain(..) {
        t.abort();
    }
}

const MAX_RECONNECT_ATTEMPTS: u32 = 5;

async fn do_emergency_disconnect(
    zenoh: &ZenohState,
    app: &AppHandle,
    device_id: String,
    message: String,
) {
    abort_sub_tasks(zenoh).await;
    stop_acquisition_child(zenoh).await;
    zenoh.node_cache.write().await.clear();

    // Enter reconnecting state instead of error
    *zenoh.connection.lock().await = ConnectionState::Reconnecting {
        device_id: device_id.clone(),
        attempt: 0,
        max_attempts: MAX_RECONNECT_ATTEMPTS,
        reason: message.clone(),
    };
    emit_connection_state(app, zenoh).await;
    let _ = app.emit("disconnect-reason", DisconnectReason { message, device_id });
}

/// Spawns a reconnection attempt loop. Called when a device is lost.
/// Watches the discovery registry for the device to reappear, then reconnects.
pub fn spawn_reconnect_task(zenoh: Arc<ZenohState>, app: AppHandle, device_id: String) {
    tauri::async_runtime::spawn(async move {
        let backoff_secs = [1, 2, 4, 8, 15];

        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            let delay = backoff_secs
                .get((attempt - 1) as usize)
                .copied()
                .unwrap_or(15);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

            // Check if user manually disconnected while we were waiting
            {
                let conn = zenoh.connection.lock().await;
                match &*conn {
                    ConnectionState::Reconnecting { .. } => {} // still us
                    _ => return,                               // user changed state, stop trying
                }
            }

            // Check if device reappeared in registry
            let found = zenoh.registry.lock().await.contains(&device_id);
            if !found {
                tracing::info!(
                    "Reconnect attempt {attempt}/{MAX_RECONNECT_ATTEMPTS}: device {device_id} not in registry"
                );
                *zenoh.connection.lock().await = ConnectionState::Reconnecting {
                    device_id: device_id.clone(),
                    attempt,
                    max_attempts: MAX_RECONNECT_ATTEMPTS,
                    reason: "Waiting for device to reappear".to_string(),
                };
                emit_connection_state(&app, &zenoh).await;
                continue;
            }

            // Device found — attempt reconnection via XML fetch
            tracing::info!("Reconnect attempt {attempt}: device {device_id} found, connecting...");
            let session = match zenoh.get_session().await {
                Ok(s) => s,
                Err(_) => {
                    tracing::warn!("No Zenoh session for reconnect");
                    break;
                }
            };

            match fetch_device_xml(&session, &device_id).await {
                Ok(xml) => {
                    let graph = match parse_genicam_xml(&xml) {
                        Ok(g) => g,
                        Err(e) => {
                            tracing::warn!("Reconnect XML parse failed: {e}");
                            continue;
                        }
                    };

                    // Re-establish subscriptions
                    abort_sub_tasks(&zenoh).await;
                    zenoh.node_cache.write().await.clear();

                    let z = zenoh.clone();
                    let tasks = vec![
                        spawn_node_value_sub(
                            session.clone(),
                            device_id.clone(),
                            z.clone(),
                            app.clone(),
                        ),
                        spawn_status_sub(
                            session.clone(),
                            device_id.clone(),
                            z.clone(),
                            app.clone(),
                        ),
                        spawn_acq_status_sub(
                            session.clone(),
                            device_id.clone(),
                            z.clone(),
                            app.clone(),
                        ),
                        spawn_image_meta_sub(
                            session.clone(),
                            device_id.clone(),
                            z.clone(),
                            app.clone(),
                        ),
                    ];
                    zenoh.sub_tasks.lock().await.extend(tasks);

                    let (device_name, device_model) = {
                        let registry = zenoh.registry.lock().await;
                        (
                            registry
                                .get_name(&device_id)
                                .unwrap_or_else(|| device_id.clone()),
                            registry.get_model(&device_id).unwrap_or_default(),
                        )
                    };

                    *zenoh.connection.lock().await = ConnectionState::Connected {
                        device_id: device_id.clone(),
                        device_name,
                        model: device_model,
                    };
                    emit_connection_state(&app, &zenoh).await;

                    let _ = app.emit(
                        "device-reconnected",
                        serde_json::json!({
                            "device_id": device_id,
                            "node_count": graph.nodes_by_name.len(),
                        }),
                    );
                    tracing::info!("Successfully reconnected to {device_id}");
                    return;
                }
                Err(e) => {
                    tracing::warn!("Reconnect XML fetch failed: {e}");
                    *zenoh.connection.lock().await = ConnectionState::Reconnecting {
                        device_id: device_id.clone(),
                        attempt,
                        max_attempts: MAX_RECONNECT_ATTEMPTS,
                        reason: format!("Reconnect failed: {e}"),
                    };
                    emit_connection_state(&app, &zenoh).await;
                }
            }
        }

        // Exhausted attempts
        tracing::warn!(
            "Reconnection to {device_id} failed after {MAX_RECONNECT_ATTEMPTS} attempts"
        );
        *zenoh.connection.lock().await = ConnectionState::Error {
            message: format!(
                "Device '{}' lost. Reconnection failed after {} attempts.",
                device_id, MAX_RECONNECT_ATTEMPTS
            ),
        };
        emit_connection_state(&app, &zenoh).await;
    });
}

pub async fn stop_acquisition_child(zenoh: &ZenohState) {
    crate::commands::acquisition::stop_streamer_tasks(zenoh).await;
}

async fn fetch_device_xml(session: &zenoh::Session, device_id: &str) -> Result<String, String> {
    let key = viva_zenoh_api::keys::xml(device_id);
    let replies = session
        .get(&key)
        .timeout(std::time::Duration::from_secs(5))
        .await
        .map_err(|e| format!("Zenoh GET error: {e}"))?;

    match replies.recv_async().await {
        Ok(reply) => match reply.result() {
            Ok(sample) => {
                let bytes = sample.payload().to_bytes();
                let resp: DeviceXmlResponse = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Invalid XML response: {e}"))?;
                Ok(resp.xml)
            }
            Err(e) => Err(format!("Reply error: {e}")),
        },
        Err(_) => Err("No reply for XML request (timeout)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiVersionStatus, check_api_version};
    use crate::state::device_state::DisconnectReason;

    #[test]
    fn test_check_api_version_compatible() {
        assert_eq!(check_api_version(Some(1), 1), ApiVersionStatus::Compatible);
    }

    #[test]
    fn test_check_api_version_missing() {
        assert_eq!(check_api_version(None, 1), ApiVersionStatus::Missing);
    }

    #[test]
    fn test_check_api_version_incompatible() {
        assert_eq!(
            check_api_version(Some(2), 1),
            ApiVersionStatus::Incompatible(2)
        );
    }

    #[test]
    fn test_disconnect_reason_serializes_fields() {
        let r = DisconnectReason {
            message: "USB link lost".to_string(),
            device_id: "cam0".to_string(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["message"], "USB link lost");
        assert_eq!(json["device_id"], "cam0");
    }

    #[test]
    fn test_disconnect_reason_empty_message() {
        let r = DisconnectReason {
            message: "".to_string(),
            device_id: "cam1".to_string(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["message"], "");
    }
}
