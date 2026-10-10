//! Feature commands, the same in both modes.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::State;
use tokio::sync::RwLock;

use crate::backend::BackendState;
use crate::error::HumanizeExt;
use crate::state::ModelState;
use crate::state::events::Events;
use viva_xml_model::{UiNode, UiNodeKind};
use viva_zenoh_api::{CommandResult, FeatureState};

/// Nodes whose state is likely to change as a side effect of executing the
/// given command, beyond what the XML declares (`NodeMap::invalidated_by`).
/// Kept deliberately small — each entry means one extra read after a command
/// succeeds.
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

/// The live state of many features: value, access mode, kind, range,
/// accepted enumeration entries and unit — the UI's single source of truth
/// (ADR-0010). Features whose state cannot be read are left out.
#[tauri::command]
pub async fn query_feature_states_bulk(
    names: Vec<String>,
    backend: State<'_, BackendState>,
) -> Result<HashMap<String, FeatureState>, String> {
    backend.bulk_feature_state(&names).await.humanize()
}

/// Validate that `value` is a legal write for `node`, against the XML's
/// declarations: kind, access along the `<pValue>` chain, static range and
/// enumeration entries.
///
/// What depends on the device's current state — locks, availability, a
/// `<pMin>`/`<pMax>` read from a register — is checked where the write is
/// carried out: by `viva-genapi` in embedded mode, by the service in remote
/// mode. Both refuse with an error that names the cause.
///
/// Returns `Ok(())` when the write is valid, or an `Err` with a human-readable message.
fn validate_node_write(node: &UiNode, value: &serde_json::Value) -> Result<(), String> {
    let access_mode = node.access_mode.as_deref().unwrap_or("RW");

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
            let c = node.constraints.as_ref();
            let (min, max, inc) = (
                c.and_then(|c| c.min),
                c.and_then(|c| c.max),
                c.and_then(|c| c.inc),
            );
            validate_numeric_constraints(min, max, inc, n, &node.name)?;
        }
        UiNodeKind::Float => {
            let n = value
                .as_f64()
                .ok_or_else(|| format!("Node '{}' requires a numeric value", node.name))?;
            let c = node.constraints.as_ref();
            let (min, max, inc) = (
                c.and_then(|c| c.min),
                c.and_then(|c| c.max),
                c.and_then(|c| c.inc),
            );
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
/// After a successful write the node is read back, because devices routinely
/// clamp or round values, and the UI reconciles its draft to what the device
/// accepted. The features the write may have changed are announced in a
/// `features-invalidated` event, for the UI to re-read the ones it shows.
#[tauri::command]
pub async fn write_node(
    node_name: String,
    value: serde_json::Value,
    model: State<'_, RwLock<ModelState>>,
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<FeatureState, String> {
    // Pre-flight against the XML, when a model is loaded.
    {
        let model_guard = model.read().await;
        if let Some(node) = model_guard
            .graph
            .as_ref()
            .and_then(|graph| graph.nodes_by_name.get(&node_name))
        {
            validate_node_write(node, &value)?;
        }
    }

    let invalidated = backend.set_feature(&node_name, &value).await.humanize()?;
    let state = backend.get_feature_state(&node_name).await.humanize()?;
    events.node_state(&node_name, &state);
    events.invalidated(&node_name, &invalidated);
    Ok(state)
}

/// Execute a Command node and return a [`CommandResult`] including the
/// refreshed state of any nodes whose value is likely to have changed as a
/// side effect (e.g. `AcquisitionStatus` after `AcquisitionStart`). The UI
/// shows a toast based on `ok`/`error` and updates its live state cache from
/// `affected_states`.
#[tauri::command]
pub async fn execute_command(
    node_name: String,
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<CommandResult, String> {
    let invalidated = match backend.exec_command(&node_name).await {
        Ok(invalidated) => invalidated,
        Err(e) => {
            return Ok(CommandResult {
                ok: false,
                error: Some(crate::error::humanize_error(&e)),
                affected_states: HashMap::new(),
            });
        }
    };
    let affected: Vec<String> = nodes_affected_by_command(&node_name)
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut affected_states = HashMap::new();
    if !affected.is_empty() {
        for (name, state) in backend
            .bulk_feature_state(&affected)
            .await
            .unwrap_or_default()
        {
            events.node_state(&name, &state);
            affected_states.insert(name, state);
        }
    }
    events.invalidated(&node_name, &invalidated);
    Ok(CommandResult {
        ok: true,
        error: None,
        affected_states,
    })
}

#[cfg(test)]
mod tests {
    use super::{nodes_affected_by_command, validate_node_write, validate_numeric_constraints};
    use viva_xml_model::{EnumEntry, NumericConstraints, UiNode, UiNodeKind};

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
        assert!(validate_node_write(&node, &serde_json::json!(512)).is_ok());
    }

    #[test]
    fn test_validate_node_write_integer_above_max() {
        let node = integer_node("Width", Some(1.0), Some(4096.0), None);
        let err = validate_node_write(&node, &serde_json::json!(5000))
            .expect_err("should reject value above max");
        assert!(err.contains("maximum"), "error: {err}");
    }

    #[test]
    fn test_validate_node_write_integer_below_min() {
        let node = integer_node("Width", Some(1.0), Some(4096.0), None);
        let err = validate_node_write(&node, &serde_json::json!(0))
            .expect_err("should reject value below min");
        assert!(err.contains("minimum"), "error: {err}");
    }

    #[test]
    fn test_validate_node_write_integer_wrong_type() {
        let node = integer_node("Width", None, None, None);
        let err = validate_node_write(&node, &serde_json::json!("hello"))
            .expect_err("should reject string for Integer");
        assert!(err.contains("integer"), "error: {err}");
    }

    // ── validate_node_write — Float ─────────────────────────────────────────────

    #[test]
    fn test_validate_node_write_float_static_range() {
        let node = float_node("ExposureTime");
        assert!(validate_node_write(&node, &serde_json::json!(50.0)).is_ok());
        let err = validate_node_write(&node, &serde_json::json!(150.0))
            .expect_err("150 is above the declared maximum");
        assert!(err.contains("exceeds maximum"), "{err}");
    }

    // ── validate_node_write — Boolean ───────────────────────────────────────────

    #[test]
    fn test_validate_node_write_boolean_wrong_type() {
        let node = bool_node("ReverseX");
        let err = validate_node_write(&node, &serde_json::json!("true"))
            .expect_err("string is not a bool");
        assert!(err.contains("boolean"), "error: {err}");
    }

    // ── validate_node_write — Enumeration ───────────────────────────────────────

    #[test]
    fn test_validate_node_write_enumeration_valid() {
        let node = enum_node("PixelFormat", &["Mono8", "Mono16", "RGB8"]);
        assert!(validate_node_write(&node, &serde_json::json!("Mono16")).is_ok());
    }

    #[test]
    fn test_validate_node_write_enumeration_invalid_entry() {
        let node = enum_node("PixelFormat", &["Mono8", "Mono16", "RGB8"]);
        let err = validate_node_write(&node, &serde_json::json!("JPEG"))
            .expect_err("unknown entry should fail");
        assert!(err.contains("JPEG"), "error: {err}");
    }

    // ── validate_node_write — access mode ───────────────────────────────────────

    #[test]
    fn test_validate_node_write_readonly_access_mode() {
        let mut node = integer_node("TemperatureAbs", None, None, None);
        node.access_mode = Some("RO".to_string());
        let err = validate_node_write(&node, &serde_json::json!(42))
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
        validate_node_write(node, &serde_json::json!(12.5))
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
            dependencies: vec![],
            dependents: vec![],
            expression: None,
            int_min: None,
            int_max: None,
            int_inc: None,
        };
        let err = validate_node_write(&node, &serde_json::json!(null))
            .expect_err("Command node should be rejected");
        assert!(
            err.contains("execute_command"),
            "error should mention execute_command: {err}"
        );
    }

    // ── affected-nodes table ────────────────────────────────────────────────

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
