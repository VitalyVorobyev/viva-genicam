//! Node value publishing, set/execute/bulk-read queryables.

use std::sync::Arc;

use genicam_zenoh_api::{
    keys, BulkReadRequest, BulkReadResponse, NodeOpResponse, NodeSetRequest, NodeValueUpdate,
};
use tokio::sync::watch;
use tracing::{debug, info, warn};
use zenoh::Session;

use crate::device::DeviceHandle;

/// Declare a queryable for node set operations.
pub async fn run_set_queryable(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::node_set(&device_id, "*");
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare node set queryable");
            return;
        }
    };
    info!(device_id, key, "node set queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let key_expr = query.key_expr().as_str();
                        let node_name = keys::extract_node_name(key_expr)
                            .unwrap_or_default()
                            .to_string();

                        let response = if node_name.is_empty() {
                            NodeOpResponse {
                                ok: false,
                                error: Some("missing node name".to_string()),
                            }
                        } else {
                            match query.payload() {
                                Some(payload) => {
                                    match serde_json::from_slice::<NodeSetRequest>(
                                        &payload.to_bytes(),
                                    ) {
                                        Ok(req) => {
                                            let value_str = req.value.to_string();
                                            // Strip quotes from string values.
                                            let value_str = value_str.trim_matches('"');
                                            match device.set_feature(&node_name, value_str).await {
                                                Ok(()) => {
                                                    debug!(device_id, node_name, "node set ok");
                                                    // Publish updated value.
                                                    if let Ok(new_val) =
                                                        device.get_feature(&node_name).await
                                                    {
                                                        publish_node_value(
                                                            &session,
                                                            &device_id,
                                                            &node_name,
                                                            &new_val,
                                                        )
                                                        .await;
                                                    }
                                                    NodeOpResponse {
                                                        ok: true,
                                                        error: None,
                                                    }
                                                }
                                                Err(e) => NodeOpResponse {
                                                    ok: false,
                                                    error: Some(e.to_string()),
                                                },
                                            }
                                        }
                                        Err(e) => NodeOpResponse {
                                            ok: false,
                                            error: Some(format!("invalid payload: {e}")),
                                        },
                                    }
                                }
                                None => NodeOpResponse {
                                    ok: false,
                                    error: Some("missing payload".to_string()),
                                },
                            }
                        };

                        let reply_key = keys::node_set(&device_id, &node_name);
                        let payload = serde_json::to_vec(&response).unwrap();
                        let _ = query.reply(&reply_key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

/// Declare a queryable for command execution.
pub async fn run_execute_queryable(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::node_execute(&device_id, "*");
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare execute queryable");
            return;
        }
    };
    info!(device_id, key, "execute queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let key_expr = query.key_expr().as_str();
                        let node_name = keys::extract_node_name(key_expr)
                            .unwrap_or_default()
                            .to_string();

                        let response = match device.exec_command(&node_name).await {
                            Ok(()) => {
                                debug!(device_id, node_name, "command executed");
                                NodeOpResponse { ok: true, error: None }
                            }
                            Err(e) => NodeOpResponse {
                                ok: false,
                                error: Some(e.to_string()),
                            },
                        };

                        let reply_key = keys::node_execute(&device_id, &node_name);
                        let payload = serde_json::to_vec(&response).unwrap();
                        let _ = query.reply(&reply_key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

/// Declare a queryable for bulk node reads.
pub async fn run_bulk_read_queryable(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::nodes_bulk_read(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare bulk read queryable");
            return;
        }
    };
    info!(device_id, key, "bulk read queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let response = match query.payload() {
                            Some(payload) => {
                                match serde_json::from_slice::<BulkReadRequest>(
                                    &payload.to_bytes(),
                                ) {
                                    Ok(req) => {
                                        let mut values = std::collections::HashMap::new();
                                        for name in &req.names {
                                            if let Ok(val) = device.get_feature(name).await {
                                                values.insert(
                                                    name.clone(),
                                                    NodeValueUpdate {
                                                        value: serde_json::Value::String(val),
                                                        access_mode: "RW".to_string(),
                                                        min: None,
                                                        max: None,
                                                        inc: None,
                                                    },
                                                );
                                            }
                                        }
                                        BulkReadResponse { values }
                                    }
                                    Err(e) => {
                                        warn!(device_id, error = %e, "invalid bulk read request");
                                        BulkReadResponse {
                                            values: std::collections::HashMap::new(),
                                        }
                                    }
                                }
                            }
                            None => BulkReadResponse {
                                values: std::collections::HashMap::new(),
                            },
                        };

                        let payload = serde_json::to_vec(&response).unwrap();
                        let _ = query.reply(&key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

/// Publish initial values for common SFNC nodes after device connection.
pub async fn publish_initial_values(
    session: &Session,
    device: &crate::device::DeviceHandle,
) {
    let device_id = device.device_id();
    // Read and publish key SFNC feature values.
    let sfnc_nodes = [
        "Width",
        "Height",
        "PixelFormat",
        "ExposureTime",
        "ExposureTimeAbs",
        "Gain",
        "GainRaw",
        "AcquisitionMode",
        "DeviceModelName",
        "DeviceVendorName",
        "DeviceSerialNumber",
        "SensorWidth",
        "SensorHeight",
        "OffsetX",
        "OffsetY",
        "BinningHorizontal",
        "BinningVertical",
    ];

    for name in &sfnc_nodes {
        if let Ok(value) = device.get_feature(name).await {
            publish_node_value(session, device_id, name, &value).await;
        }
    }
    info!(device_id, "published initial node values");
}

/// Publish a single node value update.
async fn publish_node_value(session: &Session, device_id: &str, name: &str, value: &str) {
    let update = NodeValueUpdate {
        value: serde_json::Value::String(value.to_string()),
        access_mode: "RW".to_string(),
        min: None,
        max: None,
        inc: None,
    };
    let key = keys::node_value(device_id, name);
    if let Ok(payload) = serde_json::to_vec(&update) {
        let _ = session.put(&key, payload).await;
    }
}
