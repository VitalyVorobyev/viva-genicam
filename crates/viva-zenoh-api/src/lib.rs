//! Shared Zenoh API payload types for GenICam camera services.
//!
//! This crate has no Zenoh dependency — it is a pure data contract that can be
//! used by both the camera service and any client application (e.g. a Tauri
//! desktop app or a CLI tool).

use serde::{Deserialize, Serialize};

pub mod frame_header;
pub use frame_header::{FRAME_MAGIC, FrameHeader, FrameHeaderError, HEADER_SIZE};

// ── Discovery ────────────────────────────────────────────────────────────────

/// Current GenICam Zenoh API version.
///
/// Increment this constant when making breaking changes to the Zenoh wire
/// protocol.  The service publishes this value; clients check it on discovery
/// and emit warnings when versions differ.
///
/// ## Version history
/// - `1` — initial `NodeValueUpdate` contract with optional `min`/`max`/`inc`.
/// - `2` — adds [`FeatureState`] / [`NumericRange`] / [`CommandResult`] for
///   live introspection. `NodeValueUpdate` stays wire-compatible; readers that
///   understand the new types can consume the new queryables and payloads.
pub const API_VERSION: u32 = 2;

/// Periodic announcement published by the camera service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAnnounce {
    pub id: String,
    pub name: String,
    pub model: String,
    pub serial: String,
    /// Zenoh API version supported by this service.
    ///
    /// `None` when deserializing from older services that do not include the
    /// field — handled gracefully by the app (warns but still discovers).
    #[serde(default)]
    pub api_version: Option<u32>,
}

// ── Connection Lifecycle ─────────────────────────────────────────────────────

/// Device connection status pushed by the service on change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub connected: bool,
    pub error: Option<String>,
}

/// Response to `genicam/devices/{id}/xml` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceXmlResponse {
    pub xml: String,
}

// ── Node Values ──────────────────────────────────────────────────────────────

/// Live node value update published by the service on change.
///
/// `min`, `max`, and `inc` are optional runtime constraint hints.
/// When present, the UI can tighten slider ranges without re-parsing XML.
/// Services that do not implement constraint propagation may omit them.
///
/// **Legacy payload.** New code should publish [`FeatureState`] instead — it
/// carries the same information plus runtime introspection (access mode, kind,
/// implementation/availability, available enum entries). `NodeValueUpdate`
/// stays in the wire contract so older services/clients keep working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeValueUpdate {
    pub value: serde_json::Value,
    pub access_mode: String,
    /// Optional minimum allowed value for this node at the current camera state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Optional maximum allowed value for this node at the current camera state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Optional increment (step) for this node's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inc: Option<f64>,
}

// ── FeatureState (live introspection) ────────────────────────────────────────

/// Numeric range for an Integer or Float feature at the current camera state.
///
/// `min`/`max` are resolved from the XML's static declaration, from `pMin`/`pMax`
/// references when present, or from selector-dependent rules. Services that
/// cannot resolve the range must omit [`FeatureState::numeric`] entirely rather
/// than invent defaults like `i64::MIN..=i64::MAX` (the UI renders "range
/// unknown" in that case).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NumericRange {
    pub min: f64,
    pub max: f64,
    /// Optional increment (step). Omitted when the feature has no grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inc: Option<f64>,
}

/// Live state of a GenICam feature at a single point in time.
///
/// Replaces [`NodeValueUpdate`] as the authoritative snapshot consumed by the
/// UI. Every field reflects what the device reports *now*, not what the XML
/// declares statically.
///
/// # Design notes
///
/// - `kind` is a plain string matching `viva_genapi::Node::kind_name` values
///   ("Integer", "Float", "Enumeration", "Boolean", "Command", "Category",
///   "SwissKnife", "Converter", "IntConverter", "StringReg"). Clients should
///   tolerate unknown kinds.
/// - `access_mode` uses GenICam spelling: `"RO"`, `"RW"`, `"WO"`, `"NA"`.
/// - `is_implemented` and `is_available` default to `true` for deserialization
///   from services that do not evaluate these predicates yet.
/// - `numeric` is `Some` only for Integer/Float nodes with a resolvable range.
/// - `enum_available` is `Some` only for Enumeration nodes. When the service
///   cannot filter by IsAvailable it SHOULD still populate this with the full
///   entry list so the UI stops falling back to static XML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeatureState {
    /// Current feature value, typed according to `kind`.
    pub value: serde_json::Value,
    /// Live access mode.
    pub access_mode: String,
    /// GenICam node kind.
    pub kind: String,
    /// Whether the node is implemented by the device.
    #[serde(default = "default_true")]
    pub is_implemented: bool,
    /// Whether the node is currently accessible (selector gating etc).
    #[serde(default = "default_true")]
    pub is_available: bool,
    /// Range for Integer/Float nodes, when resolvable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric: Option<NumericRange>,
    /// Available enum entries for Enumeration nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_available: Option<Vec<String>>,
    /// Engineering unit copied from node metadata (e.g. `"us"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

fn default_true() -> bool {
    true
}

impl FeatureState {
    /// Project this state into the legacy [`NodeValueUpdate`] shape so callers
    /// that still speak the old contract keep working during migration.
    pub fn to_node_value_update(&self) -> NodeValueUpdate {
        let (min, max, inc) = match &self.numeric {
            Some(r) => (Some(r.min), Some(r.max), r.inc),
            None => (None, None, None),
        };
        NodeValueUpdate {
            value: self.value.clone(),
            access_mode: self.access_mode.clone(),
            min,
            max,
            inc,
        }
    }
}

impl From<&FeatureState> for NodeValueUpdate {
    fn from(s: &FeatureState) -> Self {
        s.to_node_value_update()
    }
}

// ── CommandResult ────────────────────────────────────────────────────────────

/// Response from an `execute` queryable.
///
/// Like [`NodeOpResponse`] but also returns the post-execution state of any
/// nodes whose value is likely to have changed. For example, executing
/// `AcquisitionStart` populates `affected_states` with the refreshed state of
/// `AcquisitionStatus`, etc. The UI uses these to update badges / form inputs
/// without a second round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Map of node name -> refreshed state for nodes affected by this command.
    /// Empty when the service cannot determine side effects.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub affected_states: std::collections::HashMap<String, FeatureState>,
}

/// Request payload for the `nodes/{name}/set` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSetRequest {
    pub value: serde_json::Value,
}

/// Generic response for node write, execute, and acquisition control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOpResponse {
    pub ok: bool,
    pub error: Option<String>,
}

// ── Bulk Node Read ────────────────────────────────────────────────────────────

/// Request payload for the `nodes/bulk/read` queryable.
///
/// An empty `names` list is valid and returns an empty map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkReadRequest {
    pub names: Vec<String>,
}

/// Response to a `nodes/bulk/read` query.
///
/// `values` maps each requested node name to its current value + access_mode.
/// Node names not found in the store are omitted (not an error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkReadResponse {
    pub values: std::collections::HashMap<String, NodeValueUpdate>,
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// Request payload for the `acquisition/control` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquisitionControlRequest {
    pub command: AcquisitionCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcquisitionCommand {
    Start,
    Stop,
}

/// Acquisition status pushed by the service on change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquisitionStatus {
    pub active: bool,
    pub fps: Option<f32>,
    pub dropped: u64,
}

// ── Image ────────────────────────────────────────────────────────────────────

/// SFNC pixel format identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Mono8,
    Mono10,
    Mono12,
    Mono16,
    BayerRG8,
    BayerGR8,
    BayerBG8,
    BayerGB8,
    BayerRG10,
    BayerGR10,
    BayerBG10,
    BayerGB10,
    BayerRG12,
    BayerGR12,
    BayerBG12,
    BayerGB12,
    BayerRG16,
    BayerGR16,
    BayerBG16,
    BayerGB16,
    RGB8,
    BGR8,
    RGBa8,
    YCbCr422_8,
    YCbCr8,
    #[serde(rename = "Coord3D_C16")]
    Coord3dC16,
    #[serde(other)]
    Unknown,
}

impl PixelFormat {
    /// Bytes per pixel (or fractional for packed/subsampled formats).
    pub fn bytes_per_pixel(&self) -> f32 {
        match self {
            Self::Mono8 | Self::BayerRG8 | Self::BayerGR8 | Self::BayerBG8 | Self::BayerGB8 => 1.0,
            Self::Mono10
            | Self::Mono12
            | Self::Mono16
            | Self::BayerRG10
            | Self::BayerGR10
            | Self::BayerBG10
            | Self::BayerGB10
            | Self::BayerRG12
            | Self::BayerGR12
            | Self::BayerBG12
            | Self::BayerGB12
            | Self::BayerRG16
            | Self::BayerGR16
            | Self::BayerBG16
            | Self::BayerGB16
            | Self::Coord3dC16 => 2.0,
            Self::RGB8 | Self::BGR8 | Self::YCbCr8 => 3.0,
            Self::RGBa8 => 4.0,
            Self::YCbCr422_8 => 2.0,
            Self::Unknown => 1.0,
        }
    }
}

/// Image stream metadata published at acquisition start and on format change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMeta {
    pub pixel_format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub payload_size: u64,
}

// ── Key Expressions ──────────────────────────────────────────────────────────

/// Key expression constants and helpers for the GenICam Zenoh API.
pub mod keys {
    /// Wildcard subscription for all device announcements.
    pub const ANNOUNCE_ALL: &str = "genicam/devices/*/announce";

    pub fn announce(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/announce")
    }

    pub fn xml(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/xml")
    }

    pub fn status(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/status")
    }

    pub fn node_value(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/value")
    }

    pub fn node_value_wildcard(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/*/value")
    }

    pub fn node_set(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/set")
    }

    pub fn node_execute(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/execute")
    }

    /// Key expression for a single-node [`super::FeatureState`] queryable.
    /// Direction: App -> Service (queryable GET). Reply payload is a
    /// [`super::FeatureState`] JSON object.
    pub fn node_introspect(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/state")
    }

    /// Key expression for the bulk node read queryable.
    /// Direction: App -> Service (queryable GET).
    pub fn nodes_bulk_read(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/bulk/read")
    }

    /// Key expression for the bulk [`super::FeatureState`] queryable.
    /// Direction: App -> Service (queryable GET). Reply payload is a
    /// `HashMap<String, FeatureState>` JSON object.
    pub fn nodes_bulk_state(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/bulk/state")
    }

    pub fn acquisition_control(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/acquisition/control")
    }

    pub fn acquisition_status(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/acquisition/status")
    }

    pub fn image(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/image")
    }

    pub fn image_meta(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/image/meta")
    }

    /// Extract node name from `genicam/devices/{id}/nodes/{name}/{suffix}`
    /// where suffix is `"value"`, `"set"`, or `"execute"`.
    pub fn extract_node_name(key: &str) -> Option<&str> {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() >= 6 && parts[parts.len() - 3] == "nodes" {
            Some(parts[parts.len() - 2])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_announce_deserializes_without_api_version() {
        let legacy = r#"{"id":"cam0","name":"Test Cam","model":"M1","serial":"S1"}"#;
        let a: DeviceAnnounce = serde_json::from_str(legacy).expect("should deserialize");
        assert!(
            a.api_version.is_none(),
            "api_version should be None for legacy JSON"
        );
    }

    #[test]
    fn test_device_announce_deserializes_with_api_version() {
        let json = r#"{"id":"cam0","name":"Test","model":"M","serial":"S","api_version":1}"#;
        let a: DeviceAnnounce = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(a.api_version, Some(1));
    }

    #[test]
    fn test_node_value_update_without_constraints() {
        let u = NodeValueUpdate {
            value: serde_json::json!(42),
            access_mode: "RW".to_string(),
            min: None,
            max: None,
            inc: None,
        };
        let s = serde_json::to_string(&u).expect("serialization failed");
        assert!(!s.contains("\"min\""), "min should be absent: {s}");
        assert!(!s.contains("\"max\""), "max should be absent: {s}");
        assert!(!s.contains("\"inc\""), "inc should be absent: {s}");
        assert!(s.contains("\"value\""));
        assert!(s.contains("\"access_mode\""));
    }

    #[test]
    fn test_node_value_update_with_constraints() {
        let u = NodeValueUpdate {
            value: serde_json::json!(1024),
            access_mode: "RW".to_string(),
            min: Some(1.0),
            max: Some(4096.0),
            inc: Some(1.0),
        };
        let s = serde_json::to_string(&u).expect("serialization failed");
        let d: NodeValueUpdate = serde_json::from_str(&s).expect("deserialization failed");
        assert_eq!(d.min, Some(1.0));
        assert_eq!(d.max, Some(4096.0));
        assert_eq!(d.inc, Some(1.0));
        assert_eq!(d.access_mode, "RW");
    }

    #[test]
    fn test_extract_node_name_value_key() {
        assert_eq!(
            keys::extract_node_name("genicam/devices/cam0/nodes/Width/value"),
            Some("Width")
        );
    }

    #[test]
    fn test_extract_node_name_set_key() {
        assert_eq!(
            keys::extract_node_name("genicam/devices/cam0/nodes/Width/set"),
            Some("Width")
        );
    }

    #[test]
    fn test_extract_node_name_execute_key() {
        assert_eq!(
            keys::extract_node_name("genicam/devices/cam0/nodes/AcquisitionStart/execute"),
            Some("AcquisitionStart")
        );
    }

    #[test]
    fn test_extract_node_name_too_short() {
        assert_eq!(keys::extract_node_name("genicam/devices/cam0"), None);
    }

    #[test]
    fn test_extract_node_name_non_node_key() {
        assert_eq!(
            keys::extract_node_name("genicam/devices/cam0/acquisition/control/something"),
            None
        );
    }

    #[test]
    fn test_node_value_update_deserializes_legacy() {
        let legacy = r#"{"value": 1024, "access_mode": "RW"}"#;
        let d: NodeValueUpdate = serde_json::from_str(legacy).expect("deserialization failed");
        assert!(d.min.is_none());
        assert!(d.max.is_none());
        assert!(d.inc.is_none());
        assert_eq!(d.access_mode, "RW");
    }

    // ── FeatureState ────────────────────────────────────────────────────────

    #[test]
    fn test_feature_state_minimal_roundtrip() {
        let s = FeatureState {
            value: serde_json::json!(1920),
            access_mode: "RW".to_string(),
            kind: "Integer".to_string(),
            is_implemented: true,
            is_available: true,
            numeric: None,
            enum_available: None,
            unit: None,
        };
        let j = serde_json::to_string(&s).expect("serialize");
        // Optional fields should be skipped.
        assert!(!j.contains("\"numeric\""), "numeric should be absent: {j}");
        assert!(
            !j.contains("\"enum_available\""),
            "enum_available absent: {j}"
        );
        assert!(!j.contains("\"unit\""), "unit absent: {j}");
        let d: FeatureState = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(d, s);
    }

    #[test]
    fn test_feature_state_defaults_when_fields_missing() {
        // A service that does not yet populate is_implemented/is_available
        // must still deserialize into a usable state.
        let legacy = r#"{"value":1920,"access_mode":"RW","kind":"Integer"}"#;
        let d: FeatureState = serde_json::from_str(legacy).expect("deserialize");
        assert!(d.is_implemented, "is_implemented defaults to true");
        assert!(d.is_available, "is_available defaults to true");
    }

    #[test]
    fn test_feature_state_integer_with_range() {
        let s = FeatureState {
            value: serde_json::json!(512),
            access_mode: "RW".to_string(),
            kind: "Integer".to_string(),
            is_implemented: true,
            is_available: true,
            numeric: Some(NumericRange {
                min: 16.0,
                max: 4096.0,
                inc: Some(8.0),
            }),
            enum_available: None,
            unit: Some("px".to_string()),
        };
        let j = serde_json::to_string(&s).expect("serialize");
        let d: FeatureState = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(d, s);
    }

    #[test]
    fn test_feature_state_to_node_value_update() {
        let s = FeatureState {
            value: serde_json::json!(512),
            access_mode: "RW".to_string(),
            kind: "Integer".to_string(),
            is_implemented: true,
            is_available: true,
            numeric: Some(NumericRange {
                min: 16.0,
                max: 4096.0,
                inc: Some(8.0),
            }),
            enum_available: None,
            unit: None,
        };
        let u: NodeValueUpdate = (&s).into();
        assert_eq!(u.value, serde_json::json!(512));
        assert_eq!(u.access_mode, "RW");
        assert_eq!(u.min, Some(16.0));
        assert_eq!(u.max, Some(4096.0));
        assert_eq!(u.inc, Some(8.0));
    }

    #[test]
    fn test_feature_state_enumeration() {
        let s = FeatureState {
            value: serde_json::json!("Once"),
            access_mode: "RW".to_string(),
            kind: "Enumeration".to_string(),
            is_implemented: true,
            is_available: true,
            numeric: None,
            enum_available: Some(vec!["Off".to_string(), "Once".to_string()]),
            unit: None,
        };
        let j = serde_json::to_string(&s).expect("serialize");
        assert!(j.contains("\"enum_available\""));
        let d: FeatureState = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(
            d.enum_available.as_deref(),
            Some(&["Off".to_string(), "Once".to_string()][..])
        );
    }

    #[test]
    fn test_command_result_ok_empty() {
        let r = CommandResult {
            ok: true,
            error: None,
            affected_states: std::collections::HashMap::new(),
        };
        let j = serde_json::to_string(&r).expect("serialize");
        assert!(
            !j.contains("\"affected_states\""),
            "empty affected_states should be skipped: {j}"
        );
        assert!(
            !j.contains("\"error\""),
            "none error should be skipped: {j}"
        );
    }

    #[test]
    fn test_command_result_with_affected_states() {
        let mut affected = std::collections::HashMap::new();
        affected.insert(
            "AcquisitionStatus".to_string(),
            FeatureState {
                value: serde_json::json!(true),
                access_mode: "RO".to_string(),
                kind: "Boolean".to_string(),
                is_implemented: true,
                is_available: true,
                numeric: None,
                enum_available: None,
                unit: None,
            },
        );
        let r = CommandResult {
            ok: true,
            error: None,
            affected_states: affected,
        };
        let j = serde_json::to_string(&r).expect("serialize");
        let d: CommandResult = serde_json::from_str(&j).expect("deserialize");
        assert!(d.ok);
        assert_eq!(d.affected_states.len(), 1);
        assert!(d.affected_states.contains_key("AcquisitionStatus"));
    }
}
