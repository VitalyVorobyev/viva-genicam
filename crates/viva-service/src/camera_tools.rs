//! A camera's configuration, saved and restored, and its `<Register>` nodes,
//! read and written: the queries behind Viva Studio's configuration and
//! register tools in remote mode (backlog ST-50, ST-53).
//!
//! The configuration travels as the text of a GenApi feature file; the file
//! itself is read and written where the client runs.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};
use viva_zenoh_api::{
    ConfigurationLoadRequest, NodeOpResponse, RegisterData, RegisterWriteRequest, keys,
};
use zenoh::Session;
use zenoh::query::Query;

use crate::device::DeviceOps;

/// Reply to `query` on `key` with `payload` as JSON.
async fn reply_json(query: &Query, key: &str, payload: &impl serde::Serialize) {
    match serde_json::to_vec(payload) {
        Ok(bytes) => {
            let _ = query.reply(key, bytes).await;
        }
        Err(e) => {
            let _ = query.reply_err(format!("serialize error: {e}")).await;
        }
    }
}

/// Answer the configuration save and load queryables until `shutdown`.
pub(crate) async fn run_configuration<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let save_key = keys::configuration_save(&device_id);
    let load_key = keys::configuration_load(&device_id);
    let (save, load) = match (
        session.declare_queryable(&save_key).await,
        session.declare_queryable(&load_key).await,
    ) {
        (Ok(save), Ok(load)) => (save, load),
        (Err(e), _) | (_, Err(e)) => {
            warn!(device_id, error = %e, "failed to declare the configuration queryables");
            return;
        }
    };
    info!(
        device_id,
        save_key, load_key, "configuration queryables ready"
    );

    loop {
        tokio::select! {
            query = save.recv_async() => {
                let Ok(query) = query else { break };
                match device.save_configuration().await {
                    Ok(saved) => reply_json(&query, &save_key, &saved).await,
                    Err(e) => {
                        warn!(device_id, error = %e, "saving the configuration failed");
                        let _ = query.reply_err(e.to_string()).await;
                    }
                }
            }
            query = load.recv_async() => {
                let Ok(query) = query else { break };
                let request = query
                    .payload()
                    .ok_or_else(|| "missing payload".to_string())
                    .and_then(|payload| {
                        serde_json::from_slice::<ConfigurationLoadRequest>(&payload.to_bytes())
                            .map_err(|e| format!("invalid payload: {e}"))
                    });
                let result = match request {
                    Ok(request) => device
                        .load_configuration(request.text)
                        .await
                        .map_err(|e| e.to_string()),
                    Err(message) => Err(message),
                };
                match result {
                    Ok(loaded) => reply_json(&query, &load_key, &loaded).await,
                    Err(message) => {
                        warn!(device_id, %message, "loading the configuration failed");
                        let _ = query.reply_err(message).await;
                    }
                }
            }
            _ = crate::serve::stopped(&mut shutdown) => break,
        }
    }
}

/// Answer the register read and write queryables until `shutdown`.
pub(crate) async fn run_registers<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let read_key = keys::node_register(&device_id, "*");
    let write_key = keys::node_register_set(&device_id, "*");
    let (read, write) = match (
        session.declare_queryable(&read_key).await,
        session.declare_queryable(&write_key).await,
    ) {
        (Ok(read), Ok(write)) => (read, write),
        (Err(e), _) | (_, Err(e)) => {
            warn!(device_id, error = %e, "failed to declare the register queryables");
            return;
        }
    };
    info!(device_id, read_key, write_key, "register queryables ready");

    loop {
        tokio::select! {
            query = read.recv_async() => {
                let Ok(query) = query else { break };
                let name = keys::extract_node_name(query.key_expr().as_str())
                    .unwrap_or_default()
                    .to_string();
                match device.get_register(&name).await {
                    Ok(data) => {
                        let key = keys::node_register(&device_id, &name);
                        reply_json(&query, &key, &RegisterData::new(data)).await;
                    }
                    Err(e) => {
                        let _ = query.reply_err(format!("Failed to read '{name}': {e}")).await;
                    }
                }
            }
            query = write.recv_async() => {
                let Ok(query) = query else { break };
                let name = keys::extract_node_name(query.key_expr().as_str())
                    .unwrap_or_default()
                    .to_string();
                let request = query
                    .payload()
                    .ok_or_else(|| "missing payload".to_string())
                    .and_then(|payload| {
                        serde_json::from_slice::<RegisterWriteRequest>(&payload.to_bytes())
                            .map_err(|e| format!("invalid payload: {e}"))
                    });
                let response = match request {
                    Ok(request) => match device.set_register(&name, request.data).await {
                        Ok(()) => NodeOpResponse::ok()
                            .with_invalidated(device.invalidated_by(&name).await),
                        Err(e) => NodeOpResponse::err(format!("Failed to write '{name}': {e}")),
                    },
                    Err(message) => NodeOpResponse::err(message),
                };
                let key = keys::node_register_set(&device_id, &name);
                reply_json(&query, &key, &response).await;
            }
            _ = crate::serve::stopped(&mut shutdown) => break,
        }
    }
}
