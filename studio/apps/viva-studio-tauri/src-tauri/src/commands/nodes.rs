use std::collections::HashMap;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;

use crate::error::HumanizeExt;
use crate::state::ModelState;
use crate::state::device_state::{ConnectionState, NodeValueEntry, ZenohState};
use viva_xml_model::{UiNode, UiNodeKind};
use viva_zenoh_api::{
    BulkReadRequest, BulkReadResponse, CommandResult, FeatureState, NodeOpResponse, NodeSetRequest,
};

/// Nodes whose state is likely to change as a side effect of executing the
/// given command. Kept deliberately small — each entry means one extra read
/// after a command succeeds.
fn nodes_affected_by_command(command: &str) -> &'static [&'static str] {
    match command {
        // Acquisition lifecycle commands flip the status / running flags and
        // typically freeze frame rate until the stream is active.
        "AcquisitionStart" | "AcquisitionStop" | "AcquisitionAbort" => {
            &["AcquisitionStatus", "AcquisitionFrameRate"]
        }
        _ => &[],
    }
}

/// Emit a `node-value-changed` event carrying the legacy flat fields plus a
/// rich `state: FeatureState` member. Old consumers reading `value` /
/// `access_mode` keep working; new consumers read `state`.
fn emit_node_state(app: &AppHandle, node_name: &str, state: &FeatureState) {
    let _ = app.emit(
        "node-value-changed",
        serde_json::json!({
            "node_name": node_name,
            "value": state.value,
            "access_mode": state.access_mode,
            "state": state,
        }),
    );
}

#[tauri::command]
pub async fn get_node_value(
    node_name: String,
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
) -> Result<NodeValueEntry, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        return backend.get_feature(&node_name).await;
    }
    zenoh
        .node_cache
        .read()
        .await
        .get(&node_name)
        .cloned()
        .ok_or_else(|| format!("Node '{node_name}' not in cache"))
        .humanize()
}

/// Read the full live state of a node: value + access mode + kind + resolvable
/// range + available enum entries + unit. This is the authoritative snapshot
/// the Feature Browser UI consumes as its single source of truth.
///
/// In embedded mode the state is produced from a typed read against the
/// connected camera. In remote mode the state is projected from the Zenoh
/// `node_cache` until the service is upgraded to publish `FeatureState`
/// directly (tracked by Step 4 of the migration plan).
#[tauri::command]
pub async fn query_feature_state(
    node_name: String,
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
) -> Result<FeatureState, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        return backend.get_feature_state(&node_name).await;
    }
    let entry = zenoh
        .node_cache
        .read()
        .await
        .get(&node_name)
        .cloned()
        .ok_or_else(|| format!("Node '{node_name}' not in cache"))
        .humanize()?;
    Ok(node_value_entry_to_feature_state(&entry))
}

/// Bulk variant of [`query_feature_state`]. Names not found in the cache /
/// readable on the camera are silently omitted.
#[tauri::command]
pub async fn query_feature_states_bulk(
    names: Vec<String>,
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
) -> Result<HashMap<String, FeatureState>, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        return backend.bulk_feature_state(&names).await;
    }
    let cache = zenoh.node_cache.read().await;
    let mut out = HashMap::with_capacity(names.len());
    for name in &names {
        if let Some(entry) = cache.get(name) {
            out.insert(name.clone(), node_value_entry_to_feature_state(entry));
        }
    }
    Ok(out)
}

/// Projection used when the remote service has not yet been upgraded to
/// publish [`FeatureState`] directly. Carries the value, access mode, and any
/// runtime range hints that did make it onto the wire (ZA-06), but leaves
/// `kind` as `"Unknown"` and `enum_available` as `None` — the UI treats those
/// as "fall through to static XML". Remove once Step 4 upgrades the wire.
fn node_value_entry_to_feature_state(entry: &NodeValueEntry) -> FeatureState {
    let numeric = match (entry.min, entry.max) {
        (Some(min), Some(max)) => Some(viva_zenoh_api::NumericRange {
            min,
            max,
            inc: entry.inc,
        }),
        _ => None,
    };
    FeatureState {
        value: entry.value.clone(),
        access_mode: entry.access_mode.clone(),
        kind: "Unknown".to_string(),
        is_implemented: true,
        is_available: true,
        numeric,
        enum_available: None,
        unit: None,
    }
}

/// Validate that `value` is a legal write for `node`, using runtime constraints from `live`
/// (ZA-06) with fallback to static XML constraints.
///
/// Returns `Ok(())` when the write is valid, or an `Err` with a human-readable message.
fn validate_node_write(
    node: &UiNode,
    live: Option<&NodeValueEntry>,
    value: &serde_json::Value,
) -> Result<(), String> {
    // Access-mode check: prefer live cache, fall back to static XML declaration.
    let access_mode = live
        .map(|e| e.access_mode.as_str())
        .or(node.access_mode.as_deref())
        .unwrap_or("RW");

    if access_mode == "RO" || access_mode == "NA" {
        return Err(format!(
            "Node '{}' is not writable (access_mode={access_mode})",
            node.name
        ));
    }

    match &node.kind {
        UiNodeKind::Integer => {
            let n = value
                .as_f64()
                .ok_or_else(|| format!("Node '{}' requires an integer value", node.name))?;
            let min = live
                .and_then(|e| e.min)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.min));
            let max = live
                .and_then(|e| e.max)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.max));
            let inc = live
                .and_then(|e| e.inc)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.inc));
            validate_numeric_constraints(min, max, inc, n, &node.name)?;
        }
        UiNodeKind::Float => {
            let n = value
                .as_f64()
                .ok_or_else(|| format!("Node '{}' requires a numeric value", node.name))?;
            let min = live
                .and_then(|e| e.min)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.min));
            let max = live
                .and_then(|e| e.max)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.max));
            let inc = live
                .and_then(|e| e.inc)
                .or_else(|| node.constraints.as_ref().and_then(|c| c.inc));
            validate_numeric_constraints(min, max, inc, n, &node.name)?;
        }
        UiNodeKind::Boolean => {
            if !value.is_boolean() {
                return Err(format!(
                    "Node '{}' requires a boolean value (true/false)",
                    node.name
                ));
            }
        }
        UiNodeKind::String => {
            if !value.is_string() {
                return Err(format!("Node '{}' requires a string value", node.name));
            }
        }
        UiNodeKind::Enumeration => {
            let s = value
                .as_str()
                .ok_or_else(|| format!("Node '{}' requires a string enum entry", node.name))?;
            if !node.enum_entries.is_empty() && !node.enum_entries.iter().any(|e| e.name == s) {
                return Err(format!(
                    "'{}' is not a valid entry for enumeration node '{}'",
                    s, node.name
                ));
            }
        }
        UiNodeKind::Command => {
            return Err(format!(
                "Node '{}' is a Command node; use execute_command instead of write_node",
                node.name
            ));
        }
        UiNodeKind::Category | UiNodeKind::Register | UiNodeKind::Unknown { .. } => {
            return Err(format!(
                "Node '{}' (kind {:?}) is not writable",
                node.name, node.kind
            ));
        }
    }

    Ok(())
}

/// Check that `n` satisfies optional min/max/inc constraints.
fn validate_numeric_constraints(
    min: Option<f64>,
    max: Option<f64>,
    inc: Option<f64>,
    n: f64,
    node_name: &str,
) -> Result<(), String> {
    if let Some(min) = min
        && n < min
    {
        return Err(format!(
            "Value {n} is below minimum {min} for node '{node_name}'"
        ));
    }
    if let Some(max) = max
        && n > max
    {
        return Err(format!(
            "Value {n} exceeds maximum {max} for node '{node_name}'"
        ));
    }
    if let Some(inc) = inc
        && inc > 0.0
    {
        let base = min.unwrap_or(0.0);
        let remainder = ((n - base) / inc).fract().abs();
        // Allow a small floating-point epsilon on either side of an integer step.
        if remainder > 1e-9 && (1.0 - remainder) > 1e-9 {
            return Err(format!(
                "Value {n} is not aligned to increment {inc} for node '{node_name}'"
            ));
        }
    }
    Ok(())
}

/// Write a value and return the authoritative post-write state.
///
/// After a successful write, the backend re-reads the node to confirm what
/// the device actually accepted (devices routinely clamp or round values) and
/// returns the resulting [`FeatureState`]. The UI reconciles its draft to
/// `result.value` so the form always mirrors device truth — this is what
/// fixes the "enum form resets to (unset) after apply" bug.
#[tauri::command]
pub async fn write_node(
    node_name: String,
    value: serde_json::Value,
    zenoh: State<'_, Arc<ZenohState>>,
    model: State<'_, RwLock<ModelState>>,
    backend: State<'_, crate::backend::BackendState>,
    app: AppHandle,
) -> Result<FeatureState, String> {
    // Pre-flight: validate against UiGraph constraints when a model is loaded.
    // If no model is present (e.g., pure Zenoh mode without XML) we skip silently.
    {
        let model_guard = model.read().await;
        if let Some(graph) = &model_guard.graph
            && let Some(node) = graph.nodes_by_name.get(&node_name)
        {
            let cache = zenoh.node_cache.read().await;
            let live = cache.get(&node_name);
            validate_node_write(node, live, &value)?;
        }
    }

    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        backend.set_feature(&node_name, &value).await?;
        // Re-read the full state from the camera and notify the UI.
        let state = backend.get_feature_state(&node_name).await?;
        emit_node_state(&app, &node_name, &state);
        return Ok(state);
    }

    let session = zenoh.get_session().await.humanize()?;
    let device_id = connected_device_id(&zenoh).await.humanize()?;

    let key = viva_zenoh_api::keys::node_set(&device_id, &node_name);
    let payload = serde_json::to_vec(&NodeSetRequest { value }).map_err(|e| e.to_string())?;

    let replies = session
        .get(&key)
        .payload(payload)
        .timeout(std::time::Duration::from_secs(5))
        .await
        .map_err(|e| format!("Zenoh error: {e}"))
        .humanize()?;

    match replies.recv_async().await {
        Ok(reply) => match reply.result() {
            Ok(sample) => {
                let bytes = sample.payload().to_bytes();
                let resp: NodeOpResponse =
                    serde_json::from_slice(&bytes).map_err(|e| format!("Parse error: {e}"))?;
                if !resp.ok {
                    return Err(resp.error.unwrap_or_else(|| "Write failed".to_string()))
                        .humanize();
                }
                // Remote service has not yet been upgraded to return
                // FeatureState on write (Step 4). Project the refreshed
                // node_cache entry — the `value/subscriber` task updates
                // this after the set — with a short wait for propagation.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let entry = zenoh
                    .node_cache
                    .read()
                    .await
                    .get(&node_name)
                    .cloned()
                    .ok_or_else(|| format!("Node '{node_name}' not in cache after write"))
                    .humanize()?;
                Ok(node_value_entry_to_feature_state(&entry))
            }
            Err(e) => Err(format!("Reply error: {e}")).humanize(),
        },
        Err(_) => Err(format!("No reply for write_node '{node_name}' (timeout)")).humanize(),
    }
}

/// Execute a Command node and return a [`CommandResult`] including the
/// refreshed state of any nodes whose value is likely to have changed as a
/// side effect (e.g. `AcquisitionStatus` after `AcquisitionStart`). The UI
/// shows a toast based on `ok`/`error` and updates its live state cache from
/// `affected_states` — this is what makes the AcquisitionStart button visibly
/// "do something".
#[tauri::command]
pub async fn execute_command(
    node_name: String,
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
    app: AppHandle,
) -> Result<CommandResult, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        match backend.exec_command(&node_name).await {
            Ok(()) => {
                // Re-read the known side-effect nodes and emit per-node events
                // so the UI updates without a manual refresh.
                let affected_names: Vec<String> = nodes_affected_by_command(&node_name)
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let mut affected_states = HashMap::new();
                if !affected_names.is_empty() {
                    let map = backend
                        .bulk_feature_state(&affected_names)
                        .await
                        .unwrap_or_default();
                    for (name, state) in map {
                        emit_node_state(&app, &name, &state);
                        affected_states.insert(name, state);
                    }
                }
                Ok(CommandResult {
                    ok: true,
                    error: None,
                    affected_states,
                })
            }
            Err(e) => Ok(CommandResult {
                ok: false,
                error: Some(e),
                affected_states: HashMap::new(),
            }),
        }
    } else {
        let session = zenoh.get_session().await.humanize()?;
        let device_id = connected_device_id(&zenoh).await.humanize()?;
        let key = viva_zenoh_api::keys::node_execute(&device_id, &node_name);

        let replies = session
            .get(&key)
            .timeout(std::time::Duration::from_secs(5))
            .await
            .map_err(|e| format!("Zenoh error: {e}"))
            .humanize()?;

        match replies.recv_async().await {
            Ok(reply) => match reply.result() {
                Ok(sample) => {
                    let bytes = sample.payload().to_bytes();
                    let resp: NodeOpResponse =
                        serde_json::from_slice(&bytes).map_err(|e| format!("Parse error: {e}"))?;
                    Ok(CommandResult {
                        ok: resp.ok,
                        error: resp.error,
                        // Remote services do not yet return affected states
                        // (Step 4). Leave empty; the UI refreshes from the
                        // subscriber stream.
                        affected_states: HashMap::new(),
                    })
                }
                Err(e) => Err(format!("Reply error: {e}")).humanize(),
            },
            Err(_) => Err(format!(
                "No reply for execute_command '{node_name}' (timeout)"
            ))
            .humanize(),
        }
    }
}

/// Parse raw bytes from a `nodes/bulk/read` reply into a `NodeValueEntry` map.
///
/// Extracted as a pure function so it can be unit-tested without a Zenoh session.
fn parse_bulk_response(bytes: &[u8]) -> Result<HashMap<String, NodeValueEntry>, String> {
    let resp = serde_json::from_slice::<BulkReadResponse>(bytes)
        .map_err(|e| format!("Parse error: {e}"))?;
    Ok(resp
        .values
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                NodeValueEntry {
                    value: v.value,
                    access_mode: v.access_mode,
                    min: v.min,
                    max: v.max,
                    inc: v.inc,
                },
            )
        })
        .collect())
}

#[tauri::command]
pub async fn read_nodes_bulk(
    names: Vec<String>,
    zenoh: State<'_, Arc<ZenohState>>,
    backend: State<'_, crate::backend::BackendState>,
) -> Result<HashMap<String, NodeValueEntry>, String> {
    if matches!(backend.mode(), crate::backend::BackendMode::Embedded) {
        return backend.bulk_read(&names).await;
    }
    let session = zenoh.get_session().await.humanize()?;
    let device_id = connected_device_id(&zenoh).await.humanize()?;

    let key = viva_zenoh_api::keys::nodes_bulk_read(&device_id);
    let payload = serde_json::to_vec(&BulkReadRequest { names }).map_err(|e| e.to_string())?;

    let replies = session
        .get(&key)
        .payload(payload)
        .timeout(std::time::Duration::from_secs(5))
        .await
        .map_err(|e| format!("Zenoh error: {e}"))
        .humanize()?;

    match replies.recv_async().await {
        Ok(reply) => match reply.result() {
            Ok(sample) => {
                let bytes = sample.payload().to_bytes();
                parse_bulk_response(&bytes)
            }
            Err(e) => Err(format!("Reply error: {e}")),
        },
        Err(_) => Err("No reply for read_nodes_bulk (timeout)".to_string()),
    }
    .humanize()
}

async fn connected_device_id(zenoh: &ZenohState) -> Result<String, String> {
    match zenoh.connection.lock().await.clone() {
        ConnectionState::Connected { device_id, .. } => Ok(device_id),
        _ => Err("Not connected to a device".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        node_value_entry_to_feature_state, nodes_affected_by_command, parse_bulk_response,
        validate_node_write, validate_numeric_constraints,
    };
    use crate::state::device_state::NodeValueEntry;
    use std::collections::HashMap;
    use viva_xml_model::{EnumEntry, NumericConstraints, RawNode, UiNode, UiNodeKind};

    fn raw_node(tag: &str) -> RawNode {
        RawNode {
            tag: tag.to_string(),
            attributes: HashMap::new(),
            children_text: HashMap::new(),
        }
    }

    fn integer_node(name: &str, min: Option<f64>, max: Option<f64>, inc: Option<f64>) -> UiNode {
        UiNode {
            name: name.to_string(),
            kind: UiNodeKind::Integer,
            display_name: None,
            comment: None,
            tooltip: None,
            description: None,
            visibility: None,
            access_mode: Some("RW".to_string()),
            unit: None,
            representation: None,
            constraints: Some(NumericConstraints {
                min,
                max,
                inc,
                value: None,
            }),
            enum_entries: vec![],
            raw: raw_node("Integer"),
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        }
    }

    fn float_node(name: &str) -> UiNode {
        UiNode {
            name: name.to_string(),
            kind: UiNodeKind::Float,
            display_name: None,
            comment: None,
            tooltip: None,
            description: None,
            visibility: None,
            access_mode: Some("RW".to_string()),
            unit: None,
            representation: None,
            constraints: Some(NumericConstraints {
                min: Some(0.0),
                max: Some(100.0),
                inc: None,
                value: None,
            }),
            enum_entries: vec![],
            raw: raw_node("Float"),
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        }
    }

    fn bool_node(name: &str) -> UiNode {
        UiNode {
            name: name.to_string(),
            kind: UiNodeKind::Boolean,
            display_name: None,
            comment: None,
            tooltip: None,
            description: None,
            visibility: None,
            access_mode: Some("RW".to_string()),
            unit: None,
            representation: None,
            constraints: None,
            enum_entries: vec![],
            raw: raw_node("Boolean"),
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        }
    }

    fn enum_node(name: &str, entries: &[&str]) -> UiNode {
        UiNode {
            name: name.to_string(),
            kind: UiNodeKind::Enumeration,
            display_name: None,
            comment: None,
            tooltip: None,
            description: None,
            visibility: None,
            access_mode: Some("RW".to_string()),
            unit: None,
            representation: None,
            constraints: None,
            enum_entries: entries
                .iter()
                .map(|e| EnumEntry {
                    name: e.to_string(),
                    value: None,
                    display_name: None,
                })
                .collect(),
            raw: raw_node("Enumeration"),
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        }
    }

    // ── validate_numeric_constraints ────────────────────────────────────────────

    #[test]
    fn test_validate_numeric_constraints_no_constraints() {
        assert!(validate_numeric_constraints(None, None, None, 999.0, "X").is_ok());
    }

    #[test]
    fn test_validate_numeric_constraints_inc_violation() {
        // min=0, max=100, inc=8 — value 10 is not on the grid (0,8,16,…)
        let err = validate_numeric_constraints(Some(0.0), Some(100.0), Some(8.0), 10.0, "Width")
            .expect_err("should reject off-grid value");
        assert!(
            err.contains("increment"),
            "error should mention increment: {err}"
        );
    }

    #[test]
    fn test_validate_numeric_constraints_inc_valid() {
        // 16 is on the 0+8k grid
        assert!(
            validate_numeric_constraints(Some(0.0), Some(100.0), Some(8.0), 16.0, "Width").is_ok()
        );
    }

    // ── validate_node_write — Integer ───────────────────────────────────────────

    #[test]
    fn test_validate_node_write_integer_valid() {
        let node = integer_node("Width", Some(1.0), Some(4096.0), Some(1.0));
        assert!(validate_node_write(&node, None, &serde_json::json!(512)).is_ok());
    }

    #[test]
    fn test_validate_node_write_integer_above_max() {
        let node = integer_node("Width", Some(1.0), Some(4096.0), None);
        let err = validate_node_write(&node, None, &serde_json::json!(5000))
            .expect_err("should reject value above max");
        assert!(err.contains("maximum"), "error: {err}");
    }

    #[test]
    fn test_validate_node_write_integer_below_min() {
        let node = integer_node("Width", Some(1.0), Some(4096.0), None);
        let err = validate_node_write(&node, None, &serde_json::json!(0))
            .expect_err("should reject value below min");
        assert!(err.contains("minimum"), "error: {err}");
    }

    #[test]
    fn test_validate_node_write_integer_wrong_type() {
        let node = integer_node("Width", None, None, None);
        let err = validate_node_write(&node, None, &serde_json::json!("hello"))
            .expect_err("should reject string for Integer");
        assert!(err.contains("integer"), "error: {err}");
    }

    #[test]
    fn test_validate_node_write_float_valid_with_runtime_constraints() {
        let node = float_node("ExposureTime");
        // Runtime constraints (ZA-06) tighten the range to 100–500 µs.
        let live = NodeValueEntry {
            value: serde_json::json!(200.0),
            access_mode: "RW".to_string(),
            min: Some(100.0),
            max: Some(500.0),
            inc: None,
        };
        assert!(validate_node_write(&node, Some(&live), &serde_json::json!(300.0)).is_ok());
        let err = validate_node_write(&node, Some(&live), &serde_json::json!(50.0))
            .expect_err("runtime min should block 50.0");
        assert!(err.contains("minimum"), "error: {err}");
    }

    // ── validate_node_write — Boolean ───────────────────────────────────────────

    #[test]
    fn test_validate_node_write_boolean_wrong_type() {
        let node = bool_node("ReverseX");
        let err = validate_node_write(&node, None, &serde_json::json!("true"))
            .expect_err("string is not a bool");
        assert!(err.contains("boolean"), "error: {err}");
    }

    // ── validate_node_write — Enumeration ───────────────────────────────────────

    #[test]
    fn test_validate_node_write_enumeration_valid() {
        let node = enum_node("PixelFormat", &["Mono8", "Mono16", "RGB8"]);
        assert!(validate_node_write(&node, None, &serde_json::json!("Mono16")).is_ok());
    }

    #[test]
    fn test_validate_node_write_enumeration_invalid_entry() {
        let node = enum_node("PixelFormat", &["Mono8", "Mono16", "RGB8"]);
        let err = validate_node_write(&node, None, &serde_json::json!("JPEG"))
            .expect_err("unknown entry should fail");
        assert!(err.contains("JPEG"), "error: {err}");
    }

    // ── validate_node_write — access mode ───────────────────────────────────────

    #[test]
    fn test_validate_node_write_readonly_access_mode() {
        let mut node = integer_node("TemperatureAbs", None, None, None);
        node.access_mode = Some("RO".to_string());
        let err = validate_node_write(&node, None, &serde_json::json!(42))
            .expect_err("RO node should be rejected");
        assert!(err.contains("not writable"), "error: {err}");
    }

    /// A feature that is itself a `<Converter>` is written through its
    /// `<pValue>`, so it is writable when that register is — the pre-flight
    /// check must not refuse what the camera would accept.
    #[test]
    fn test_validate_node_write_accepts_a_converter_over_a_writable_register() {
        let graph = viva_xml_model::parse_genicam_xml(
            r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Category Name="Root"><pFeature>AcquisitionFrameRateAbs</pFeature></Category>
                <IntReg Name="FrameRateRaw">
                    <Address>0x1000</Address><Length>4</Length><AccessMode>RW</AccessMode>
                </IntReg>
                <Converter Name="AcquisitionFrameRateAbs">
                    <FormulaTo>FROM * 100</FormulaTo>
                    <FormulaFrom>TO / 100</FormulaFrom>
                    <pValue>FrameRateRaw</pValue>
                </Converter>
            </RegisterDescription>"#,
        )
        .expect("parse");
        let node = &graph.nodes_by_name["AcquisitionFrameRateAbs"];
        validate_node_write(node, None, &serde_json::json!(12.5))
            .expect("a converter over a writable register is writable");
    }

    // ── validate_node_write — Command ───────────────────────────────────────────

    #[test]
    fn test_validate_node_write_command_rejected() {
        let node = UiNode {
            name: "AcquisitionStart".to_string(),
            kind: UiNodeKind::Command,
            display_name: None,
            comment: None,
            tooltip: None,
            description: None,
            visibility: None,
            access_mode: Some("WO".to_string()),
            unit: None,
            representation: None,
            constraints: None,
            enum_entries: vec![],
            raw: raw_node("Command"),
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        };
        let err = validate_node_write(&node, None, &serde_json::json!(null))
            .expect_err("Command node should be rejected");
        assert!(
            err.contains("execute_command"),
            "error should mention execute_command: {err}"
        );
    }

    #[test]
    fn test_parse_bulk_response_happy_path() {
        let json = r#"{
            "values": {
                "Width": {"value": 1920, "access_mode": "RW"},
                "Height": {"value": 1080, "access_mode": "RO"}
            }
        }"#;
        let result = parse_bulk_response(json.as_bytes()).expect("should parse successfully");
        assert_eq!(result.len(), 2);
        let width = result.get("Width").expect("Width should be present");
        assert_eq!(width.value, serde_json::json!(1920));
        assert_eq!(width.access_mode, "RW");
        let height = result.get("Height").expect("Height should be present");
        assert_eq!(height.value, serde_json::json!(1080));
        assert_eq!(height.access_mode, "RO");
    }

    #[test]
    fn test_parse_bulk_response_empty() {
        let json = r#"{"values": {}}"#;
        let result =
            parse_bulk_response(json.as_bytes()).expect("empty map should parse without error");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_bulk_response_invalid_json() {
        let err = parse_bulk_response(b"not json").expect_err("invalid JSON should return Err");
        assert!(
            err.contains("Parse error"),
            "error message should contain 'Parse error', got: {err}"
        );
    }

    // ── FeatureState projection / affected-nodes tables ─────────────────────

    #[test]
    fn test_node_value_entry_to_feature_state_without_range() {
        let entry = NodeValueEntry {
            value: serde_json::json!("Once"),
            access_mode: "RW".to_string(),
            min: None,
            max: None,
            inc: None,
        };
        let state = node_value_entry_to_feature_state(&entry);
        assert_eq!(state.kind, "Unknown");
        assert!(state.numeric.is_none());
        assert!(state.enum_available.is_none());
        assert_eq!(state.value, serde_json::json!("Once"));
    }

    #[test]
    fn test_node_value_entry_to_feature_state_with_range() {
        let entry = NodeValueEntry {
            value: serde_json::json!(1920),
            access_mode: "RW".to_string(),
            min: Some(16.0),
            max: Some(4096.0),
            inc: Some(8.0),
        };
        let state = node_value_entry_to_feature_state(&entry);
        let numeric = state.numeric.expect("numeric should be populated");
        assert_eq!(numeric.min, 16.0);
        assert_eq!(numeric.max, 4096.0);
        assert_eq!(numeric.inc, Some(8.0));
    }

    #[test]
    fn test_nodes_affected_by_command_acquisition_start() {
        let affected = nodes_affected_by_command("AcquisitionStart");
        assert!(affected.contains(&"AcquisitionStatus"));
    }

    #[test]
    fn test_nodes_affected_by_command_unknown() {
        let affected = nodes_affected_by_command("UserCommand123");
        assert!(affected.is_empty());
    }
}
