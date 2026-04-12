//! Node value publishing, set/execute/bulk-read queryables.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, info, warn};
use viva_zenoh_api::{
    BulkReadRequest, BulkReadResponse, FeatureState, NodeOpResponse, NodeSetRequest,
    NodeValueUpdate, keys,
};
use zenoh::Session;

use crate::device::DeviceOps;

/// Declare a queryable for node set operations.
pub async fn run_set_queryable<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
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
                                                    // Publish refreshed state (both legacy value
                                                    // and new introspect key) so subscribers see
                                                    // the post-write confirmation.
                                                    if let Ok(state) = device
                                                        .get_feature_state(&node_name)
                                                        .await
                                                    {
                                                        publish_node_state(
                                                            &session,
                                                            &device_id,
                                                            &node_name,
                                                            &state,
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
                        let Ok(payload) = serde_json::to_vec(&response) else {
                            tracing::error!("failed to serialize node set response");
                            continue;
                        };
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
pub async fn run_execute_queryable<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
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
                        let Ok(payload) = serde_json::to_vec(&response) else {
                            tracing::error!("failed to serialize execute response");
                            continue;
                        };
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

/// Declare a queryable for single-node [`FeatureState`] introspection.
///
/// Reply is a full `FeatureState` JSON object produced by
/// [`DeviceOps::get_feature_state`]. Clients (e.g. the Studio UI) use this to
/// drive slider ranges, enum dropdowns, and access-gating without falling
/// back to static XML defaults.
pub async fn run_introspect_queryable<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::node_introspect(&device_id, "*");
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare introspect queryable");
            return;
        }
    };
    info!(device_id, key, "introspect queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let key_expr = query.key_expr().as_str();
                        let node_name = keys::extract_node_name(key_expr)
                            .unwrap_or_default()
                            .to_string();

                        let reply_key = keys::node_introspect(&device_id, &node_name);
                        let payload_bytes = match device.get_feature_state(&node_name).await {
                            Ok(state) => match serde_json::to_vec(&state) {
                                Ok(b) => Some(b),
                                Err(e) => {
                                    warn!(device_id, node_name, error = %e, "failed to serialize FeatureState");
                                    None
                                }
                            },
                            Err(e) => {
                                warn!(device_id, node_name, error = %e, "introspect failed");
                                None
                            }
                        };
                        if let Some(payload) = payload_bytes {
                            let _ = query.reply(&reply_key, payload).await;
                        }
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

/// Declare a queryable for bulk [`FeatureState`] introspection.
///
/// Reply is a `HashMap<String, FeatureState>` JSON object. Reuses the same
/// `BulkReadRequest` payload shape as `nodes/bulk/read` so existing
/// producer/consumer patterns apply.
pub async fn run_bulk_state_queryable<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::nodes_bulk_state(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare bulk state queryable");
            return;
        }
    };
    info!(device_id, key, "bulk state queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let values = match query.payload() {
                            Some(payload) => {
                                match serde_json::from_slice::<BulkReadRequest>(
                                    &payload.to_bytes(),
                                ) {
                                    Ok(req) => {
                                        let mut out = std::collections::HashMap::new();
                                        for name in &req.names {
                                            if let Ok(state) = device.get_feature_state(name).await {
                                                out.insert(name.clone(), state);
                                            }
                                        }
                                        out
                                    }
                                    Err(e) => {
                                        warn!(device_id, error = %e, "invalid bulk state request");
                                        std::collections::HashMap::new()
                                    }
                                }
                            }
                            None => std::collections::HashMap::new(),
                        };

                        let Ok(payload) = serde_json::to_vec(&values) else {
                            tracing::error!("failed to serialize bulk state response");
                            continue;
                        };
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

/// Declare a queryable for bulk node reads.
pub async fn run_bulk_read_queryable<D: DeviceOps>(
    session: Arc<Session>,
    device: Arc<D>,
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
                                        // Populate the legacy `NodeValueUpdate` shape from the
                                        // rich `FeatureState` so existing clients receive real
                                        // `access_mode` + range info (ZA-06) instead of
                                        // hardcoded `"RW"` / `None`. Clients that understand
                                        // the new contract should query `nodes/bulk/state`.
                                        let mut values = std::collections::HashMap::new();
                                        for name in &req.names {
                                            if let Ok(state) = device.get_feature_state(name).await
                                            {
                                                values.insert(
                                                    name.clone(),
                                                    state.to_node_value_update(),
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

                        let Ok(payload) = serde_json::to_vec(&response) else {
                            tracing::error!("failed to serialize bulk read response");
                            continue;
                        };
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
pub async fn publish_initial_values<D: DeviceOps>(session: &Session, device: &D) {
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
        if let Ok(state) = device.get_feature_state(name).await {
            publish_node_state(session, device_id, name, &state).await;
        }
    }
    info!(device_id, "published initial node values");
}

/// Publish a single node value + legacy update payload.
///
/// Writes the legacy [`NodeValueUpdate`] to `nodes/{name}/value` (backward
/// compatibility) and also publishes the richer [`FeatureState`] to
/// `nodes/{name}/state` so introspection-aware clients get full fidelity on
/// the subscription stream as well as the queryable.
async fn publish_node_state(session: &Session, device_id: &str, name: &str, state: &FeatureState) {
    let update: NodeValueUpdate = state.into();
    let value_key = keys::node_value(device_id, name);
    if let Ok(payload) = serde_json::to_vec(&update) {
        let _ = session.put(&value_key, payload).await;
    }
    let state_key = keys::node_introspect(device_id, name);
    if let Ok(payload) = serde_json::to_vec(state) {
        let _ = session.put(&state_key, payload).await;
    }
}
