#![cfg_attr(docsrs, feature(doc_cfg))]
//! The message payloads and topic names the viva GenICam camera services
//! publish, and their clients subscribe to.
//!
//! This is the *contract*, not the transport. Despite the crate name, nothing
//! here depends on the `zenoh` crate: these are plain serde types, so a client
//! can agree with the service on the message format without linking a message
//! broker — and the crate compiles everywhere, including `wasm32`.

use serde::{Deserialize, Serialize};

pub mod evs_header;
pub mod frame_header;
pub mod frame_metadata;
pub use evs_header::{EVS_HEADER_SIZE, EVS_MAGIC, EvsFormat, EvsHeader, EvsHeaderError};
pub use frame_header::{FRAME_MAGIC, FrameHeader, FrameHeaderError, HEADER_SIZE};
pub use frame_metadata::{
    FrameChunk, FrameFeature, FrameMetadata, MAX_FRAME_METADATA_BYTES, MAX_FRAME_METADATA_ITEMS,
    MAX_FRAME_METADATA_TEXT,
};

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
/// - `3` — [`DeviceAnnounce`] carries the device's own identity (`ip`, `mac`,
///   `user_name`, `manufacturer`), and `serial` is the serial the device
///   reports rather than the service's device id
///   ([#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)). The
///   new fields are optional, so a version 2 announce still deserialises.
/// - `4` — event-vision blocks move off [`keys::image`] onto their own key,
///   [`keys::evs`], framed by an [`EvsHeader`]
///   ([#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)).
///   Nothing else changes, but a version 3 client subscribed only to `image`
///   no longer receives anything from an event camera, and the version is
///   what tells it why.
/// - `5` — a pixel format is `viva-pfnc`'s: [`ImageMeta`] names any PFNC
///   format by its PFNC name (`RGB8Packed` where version 4 said `RGB8`,
///   `Coord3D_ABC32f` where it said `Unknown`), and a USB3 Vision camera's
///   device id names its vendor, product and serial ([`u3v_device_id`]).
///   The frame header's one-byte codes are unchanged.
/// - `6` — [`AcquisitionStatus`] carries the stream's counters,
///   [`StreamCounters`], in `stats`, where version 5 had one `dropped` count;
///   it is sent every second while acquiring, frames or not, and a final
///   [`AcquisitionStatus::stopped`] snapshot follows the stream's end.
/// - `7` — [`FrameHeader`] version 2 carries the row stride, so padded images
///   retain their row layout in remote consumers. The header is 20 bytes;
///   version-1 image headers are rejected.
/// - `8` — version-3 image headers carry bounded per-frame metadata inline,
///   preserving exact identifiers, timestamps and chunk values with the pixels.
pub const API_VERSION: u32 = 8;

/// Periodic announcement published by the camera service.
///
/// Carries enough of the device's identity for a client to tell two cameras of
/// the same model apart: the user-defined name, the serial and the address.
///
/// `#[non_exhaustive]`: build one with [`DeviceAnnounce::new`] and assign the
/// optional fields, so that adding the next field is not a breaking change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceAnnounce {
    /// Service-assigned device id, also the `{id}` segment of every key
    /// expression in [`keys`]. For GigE Vision it is [`gige_device_id`].
    pub id: String,
    /// Human-readable name: the user-defined name, else the model.
    pub name: String,
    /// The camera's model name.
    pub model: String,
    /// Serial number as the device reports it; empty when it reports none.
    ///
    /// Services before API version 3 put the device id here instead.
    pub serial: String,
    /// Zenoh API version supported by this service.
    ///
    /// `None` when deserializing from older services that do not include the
    /// field — handled gracefully by the app (warns but still discovers).
    #[serde(default)]
    pub api_version: Option<u32>,
    /// Current IPv4 address in dotted-quad form. `None` for transports without
    /// one (USB3 Vision) and from services before API version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// MAC address as `AA:BB:CC:DD:EE:FF`. `None` for transports without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// User-defined device name (the GigE Vision Discovery ACK's user-defined
    /// name field), when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// Manufacturer name, when the device reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
}

impl DeviceAnnounce {
    /// An announce carrying the required fields and the current
    /// [`API_VERSION`], with every optional identity field unset.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        model: impl Into<String>,
        serial: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            model: model.into(),
            serial: serial.into(),
            api_version: Some(API_VERSION),
            ip: None,
            mac: None,
            user_name: None,
            manufacturer: None,
        }
    }

    /// The announce for a GigE Vision camera, from the fields of its Discovery
    /// ACK.
    ///
    /// The one place the GigE identity rule lives, so the service and Studio's
    /// embedded backend announce the same camera identically: `id` is
    /// [`gige_device_id`], `name` is the user-defined name, else the model,
    /// else the address, and `serial` is the device's own (empty when absent).
    pub fn for_gige(
        mac: &[u8; 6],
        ip: std::net::Ipv4Addr,
        model: Option<&str>,
        serial: Option<&str>,
        user_name: Option<&str>,
        manufacturer: Option<&str>,
    ) -> Self {
        let name = user_name
            .or(model)
            .map_or_else(|| ip.to_string(), str::to_string);
        let mut announce = Self::new(
            gige_device_id(mac),
            name,
            model.unwrap_or_default(),
            serial.unwrap_or_default(),
        );
        announce.ip = Some(ip.to_string());
        announce.mac = Some(
            mac.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":"),
        );
        announce.user_name = user_name.map(str::to_string);
        announce.manufacturer = manufacturer.map(str::to_string);
        announce
    }
}

/// Device id for a GigE Vision camera: `cam-` followed by its MAC address as
/// twelve lowercase hex digits.
///
/// The MAC is the one identifier every GigE Vision device reports, that is
/// unique on the wire, and that survives an address change — DHCP, `FORCEIP`,
/// or a persistent IP written from Studio itself. The IP is none of those, and
/// the serial is optional in the Discovery ACK. The service and Studio's
/// embedded backend both derive the id here, so a camera keeps one identity in
/// either mode.
pub fn gige_device_id(mac: &[u8; 6]) -> String {
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    format!("cam-{hex}")
}

/// Device id for a USB3 Vision camera: `cam-u3v-`, its vendor and product
/// ids as eight hex digits, then its serial number.
///
/// The serial is the USB serial string the operating system caches, which a
/// rescan reads without opening the device — so it does not change while the
/// camera is in use — and which stays the same when the camera is plugged in
/// again, wherever (backlog UV-03). On the one camera this has met, a MATRIX
/// VISION mvBlueFOX3-2004G, it is the serial its bootstrap registers report
/// too. A camera without one is named by its `bus` and `address`, and may get
/// a new id when plugged in again.
///
/// Letters, digits, `-` and `.` are kept; every other byte of the serial's
/// UTF-8, `_` included, becomes `_` and two lowercase hex digits (`/` is
/// `_2f`, `_` is `_5f`). The escape is reversible, so two serials never share
/// an id: replacing each by a bare `_` made `a/b` and `a*b` one camera
/// (Codex review of #205).
pub fn u3v_device_id(
    vendor_id: u16,
    product_id: u16,
    serial: Option<&str>,
    bus: &str,
    address: u8,
) -> String {
    let instance = match serial.map(str::trim).filter(|s| !s.is_empty()) {
        Some(serial) => serial.to_string(),
        None => format!("{bus}-{address}"),
    };
    let mut escaped = String::with_capacity(instance.len());
    for byte in instance.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' => escaped.push(char::from(byte)),
            _ => escaped.push_str(&format!("_{byte:02x}")),
        }
    }
    format!("cam-u3v-{vendor_id:04x}{product_id:04x}-{escaped}")
}

// ── Connection Lifecycle ─────────────────────────────────────────────────────

// Every payload below is `#[non_exhaustive]` and built with its constructor,
// so a field added later is not a breaking change for the services and
// clients that build them (API-13). Deserialization is unaffected.

/// Device connection status pushed by the service on change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceStatus {
    /// Whether the service holds the camera.
    pub connected: bool,
    /// Why it does not, when it does not.
    pub error: Option<String>,
}

impl DeviceStatus {
    /// The device is connected.
    pub fn connected() -> Self {
        Self {
            connected: true,
            error: None,
        }
    }

    /// The device is not connected, and why, if known.
    pub fn disconnected(error: Option<String>) -> Self {
        Self {
            connected: false,
            error,
        }
    }
}

/// Response to `genicam/devices/{id}/xml` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceXmlResponse {
    /// The camera's GenICam XML.
    pub xml: String,
}

impl DeviceXmlResponse {
    /// The device's GenApi XML.
    pub fn new(xml: impl Into<String>) -> Self {
        Self { xml: xml.into() }
    }
}

/// Request payload for the `genicam/devices/{id}/report` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceReportRequest {
    /// Include the device's GenApi XML; on unless the client asks otherwise.
    #[serde(default = "default_true")]
    pub include_xml: bool,
}

impl DeviceReportRequest {
    /// A report request, with or without the GenApi XML.
    pub fn new(include_xml: bool) -> Self {
        Self { include_xml }
    }
}

/// Response to the `report` queryable: the diagnostic report the service
/// wrote for its device, as plain text, the same report `viva-camctl report`
/// writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceReportResponse {
    /// The report, as `viva-camctl report` writes it.
    pub report: String,
}

impl DeviceReportResponse {
    /// The report text.
    pub fn new(report: impl Into<String>) -> Self {
        Self {
            report: report.into(),
        }
    }
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
#[non_exhaustive]
pub struct NodeValueUpdate {
    /// The value, typed as JSON (`null` for one that cannot be read).
    pub value: serde_json::Value,
    /// Access mode: `RO`, `RW`, `WO` or `NA`.
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

impl NodeValueUpdate {
    /// A value and its access mode, with no constraint hints.
    pub fn new(value: serde_json::Value, access_mode: impl Into<String>) -> Self {
        Self {
            value,
            access_mode: access_mode.into(),
            min: None,
            max: None,
            inc: None,
        }
    }
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[non_exhaustive]
pub struct NumericRange {
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// Optional increment (step). Omitted when the feature has no grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inc: Option<f64>,
}

impl NumericRange {
    /// A range, with an increment when the feature has a grid.
    pub fn new(min: f64, max: f64, inc: Option<f64>) -> Self {
        Self { min, max, inc }
    }
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
///   "SwissKnife", "Converter", "IntConverter", "StringReg", "Register").
///   Clients should
///   tolerate unknown kinds.
/// - `access_mode` uses GenICam spelling: `"RO"`, `"RW"`, `"WO"`, `"NA"`.
/// - `is_implemented` and `is_available` default to `true` for deserialization
///   from services that do not evaluate these predicates yet.
/// - `numeric` is `Some` only for Integer/Float nodes with a resolvable range.
/// - `enum_available` is `Some` only for Enumeration nodes. When the service
///   cannot filter by IsAvailable it SHOULD still populate this with the full
///   entry list so the UI stops falling back to static XML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[non_exhaustive]
pub struct FeatureState {
    /// Current feature value, typed according to `kind`.
    ///
    /// `null` when the feature has no value to read: a Category, a Command, a
    /// node declared `WO`, or the register a command writes through. Such a
    /// feature is still reported, with its access mode, so a client can offer
    /// the write or the execute without a value to show.
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
    /// How long, in milliseconds, the value may be shown before it is read
    /// again: the `<PollingTime>` of the feature or of a register its value
    /// is read through. Absent when nothing polls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling_ms: Option<u64>,
    /// The feature's caching mode in GenICam spelling — `"NoCache"`,
    /// `"WriteThrough"`, `"WriteAround"` — its own combined with that of the
    /// registers it is read through. A `NoCache` feature changes on the
    /// device with nothing to announce it. Absent from services that predate
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caching: Option<String>,
}

fn default_true() -> bool {
    true
}

impl FeatureState {
    /// An implemented, available feature with a value, an access mode and a
    /// kind, and no range, entries or unit.
    pub fn new(
        value: serde_json::Value,
        access_mode: impl Into<String>,
        kind: impl Into<String>,
    ) -> Self {
        Self {
            value,
            access_mode: access_mode.into(),
            kind: kind.into(),
            is_implemented: true,
            is_available: true,
            numeric: None,
            enum_available: None,
            unit: None,
            polling_ms: None,
            caching: None,
        }
    }

    /// Project this state into the legacy [`NodeValueUpdate`] shape so callers
    /// that still speak the old contract keep working during migration.
    pub fn to_node_value_update(&self) -> NodeValueUpdate {
        let (min, max, inc) = match &self.numeric {
            Some(r) => (Some(r.min), Some(r.max), r.inc),
            None => (None, None, None),
        };
        let mut update = NodeValueUpdate::new(self.value.clone(), self.access_mode.clone());
        update.min = min;
        update.max = max;
        update.inc = inc;
        update
    }
}

impl From<&FeatureState> for NodeValueUpdate {
    fn from(s: &FeatureState) -> Self {
        s.to_node_value_update()
    }
}

/// The wire form of a state read through `viva-genapi`.
///
/// Every service that holds a nodemap builds its [`FeatureState`] this way,
/// so the access-mode spelling, the register summary and the unreadable-value
/// rules cannot drift between them (backlog ST-26).
#[cfg(feature = "genapi")]
#[cfg_attr(docsrs, doc(cfg(feature = "genapi")))]
impl From<viva_genapi::FeatureState> for FeatureState {
    fn from(state: viva_genapi::FeatureState) -> Self {
        use viva_genapi::{AccessMode, FeatureValue};

        let value = match state.value {
            FeatureValue::None => serde_json::Value::Null,
            FeatureValue::Integer(v) => serde_json::Value::Number(v.into()),
            FeatureValue::Float(v) => f64_to_json(v),
            FeatureValue::Boolean(v) => serde_json::Value::Bool(v),
            FeatureValue::Enum(v) | FeatureValue::String(v) => serde_json::Value::String(v),
            FeatureValue::Register { len } => {
                serde_json::json!({ "kind": "register", "length": len })
            }
            // `FeatureValue` is `#[non_exhaustive]`: name what we cannot show
            // rather than emitting a bare null.
            _ => serde_json::json!({ "kind": state.kind, "unsupported": true }),
        };
        let access_mode = match state.access {
            Some(AccessMode::RO) => "RO",
            Some(AccessMode::RW) => "RW",
            Some(AccessMode::WO) => "WO",
            None => "NA",
        };
        FeatureState {
            value,
            access_mode: access_mode.to_string(),
            kind: state.kind.to_string(),
            is_implemented: state.is_implemented,
            is_available: state.is_available,
            numeric: state.range.map(|r| NumericRange {
                min: r.min,
                max: r.max,
                inc: r.inc,
            }),
            enum_available: state.enum_entries,
            unit: state.unit,
            polling_ms: state
                .polling_time
                .map(|t| u64::try_from(t.as_millis()).unwrap_or(u64::MAX)),
            caching: Some(caching_text(state.caching).to_string()),
        }
    }
}

/// GenICam's spelling of a caching mode.
#[cfg(feature = "genapi")]
fn caching_text(mode: viva_genapi::CachingMode) -> &'static str {
    match mode {
        viva_genapi::CachingMode::NoCache => "NoCache",
        viva_genapi::CachingMode::WriteThrough => "WriteThrough",
        viva_genapi::CachingMode::WriteAround => "WriteAround",
    }
}

/// A JSON number, or the value's text for NaN and infinities, which JSON
/// cannot carry.
#[cfg(feature = "genapi")]
fn f64_to_json(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v)
        .map(serde_json::Value::Number)
        .unwrap_or_else(|| serde_json::Value::String(v.to_string()))
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[non_exhaustive]
pub struct CommandResult {
    /// Whether the command was executed.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Why not, when it was not.
    pub error: Option<String>,
    /// Map of node name -> refreshed state for nodes affected by this command.
    /// Empty when the service cannot determine side effects.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    // Omitted when empty; `nullable` adds the `?` without unwrapping a type
    // that is not an `Option`.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub affected_states: std::collections::HashMap<String, FeatureState>,
}

impl CommandResult {
    /// Success, with no affected states.
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            affected_states: std::collections::HashMap::new(),
        }
    }

    /// Failure, with its reason.
    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            affected_states: std::collections::HashMap::new(),
        }
    }
}

/// Request payload for the `nodes/{name}/set` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodeSetRequest {
    /// The value to write, typed as JSON.
    pub value: serde_json::Value,
}

impl NodeSetRequest {
    /// Write `value`.
    pub fn new(value: serde_json::Value) -> Self {
        Self { value }
    }
}

/// Generic response for node write, execute, and acquisition control.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodeOpResponse {
    /// Whether the operation succeeded.
    pub ok: bool,
    /// Why not, when it did not.
    pub error: Option<String>,
    /// After a successful write or execute: the features whose value or
    /// state the operation may have changed, from
    /// `viva_genapi::NodeMap::invalidated_by`. A client re-reads the ones it
    /// shows. It can be hundreds of names, so the service names them rather
    /// than publishing their state. Empty for acquisition control, and from a
    /// service that predates the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalidated: Vec<String>,
}

impl NodeOpResponse {
    /// Success, with nothing invalidated.
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            invalidated: Vec::new(),
        }
    }

    /// Failure, with its reason.
    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            invalidated: Vec::new(),
        }
    }

    /// The same response, naming the features the operation invalidated.
    pub fn with_invalidated(mut self, invalidated: Vec<String>) -> Self {
        self.invalidated = invalidated;
        self
    }
}

// ── Bulk Node Read ────────────────────────────────────────────────────────────

/// Request payload for the `nodes/bulk/read` queryable.
///
/// An empty `names` list is valid and returns an empty map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BulkReadRequest {
    /// The features to read.
    pub names: Vec<String>,
}

impl BulkReadRequest {
    /// Read these features.
    pub fn new(names: Vec<String>) -> Self {
        Self { names }
    }
}

/// Response to a `nodes/bulk/read` query.
///
/// `values` maps each requested node name to its current value + access_mode.
/// Node names not found in the store are omitted (not an error).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BulkReadResponse {
    /// Their values, by name.
    pub values: std::collections::HashMap<String, NodeValueUpdate>,
}

impl BulkReadResponse {
    /// The values read, by feature name.
    pub fn new(values: std::collections::HashMap<String, NodeValueUpdate>) -> Self {
        Self { values }
    }
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// Request payload for the `acquisition/control` queryable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcquisitionControlRequest {
    /// Start or stop.
    pub command: AcquisitionCommand,
    /// For `Start`: the GVSP packet size to use, as a ceiling the service's
    /// path probe stays under. Absent, the service negotiates one from the
    /// NIC's MTU. A size the user measured for their link is how they get
    /// past a link the negotiation misjudges (issue #112).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_size: Option<u32>,
}

impl AcquisitionControlRequest {
    /// `command` with the service's own choice of packet size.
    pub fn new(command: AcquisitionCommand) -> Self {
        Self {
            command,
            packet_size: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// What an acquisition control request asks for.
pub enum AcquisitionCommand {
    /// Start acquisition and stream.
    Start,
    /// Stop streaming and acquisition.
    Stop,
}

/// Acquisition status pushed by the service: once when acquisition starts,
/// once a second while it runs, whether or not frames arrive, a final
/// [`stopped`](Self::stopped) snapshot when the stream ends, and
/// [`inactive`](Self::inactive) once the camera has stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct AcquisitionStatus {
    /// Whether the camera is acquiring.
    pub active: bool,
    /// Frames a second delivered by the stream over the last status
    /// interval, when known. It counts every frame the stream hands over,
    /// not the ones a viewer draws.
    pub fps: Option<f32>,
    /// The stream's counters since it started; `null` on the wire when
    /// there are none: in the status sent on start, before the first count,
    /// and in the one that says acquisition is off.
    pub stats: Option<StreamCounters>,
}

impl AcquisitionStatus {
    /// Acquiring at `fps`, with the stream's counters so far.
    pub fn active(fps: Option<f32>, stats: Option<StreamCounters>) -> Self {
        Self {
            active: true,
            fps,
            stats,
        }
    }

    /// Not acquiring, and nothing to count.
    pub fn inactive() -> Self {
        Self {
            active: false,
            fps: None,
            stats: None,
        }
    }

    /// The stream has ended; `stats` are its final counters.
    pub fn stopped(stats: StreamCounters) -> Self {
        Self {
            active: false,
            fps: None,
            stats: Some(stats),
        }
    }
}

/// One stream's counters on the wire: `viva_genicam::StreamStats`, with the
/// six reasons a packet was ignored summed into one.
///
/// **USB3 Vision** streams fill the frame counters the same way. Of the packet
/// counters, `packets_received` counts bulk transfers and `packets_ignored`
/// the transfers that were not the leader, payload or trailer due;
/// `transfer_errors` counts failed transfers the stream recovered from. The
/// other packet and resend counters stay 0.
///
/// Every block the camera sends is counted once, when its fate is decided:
/// complete, incomplete, missed, dropped to backpressure or unsupported. A
/// frame still waiting in the delivery queue is in none of them yet.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct StreamCounters {
    /// Frames handed to the application with every packet present.
    pub frames_complete: u64,
    /// Frames closed without that: packets missing, no trailer, or an error
    /// status from the device. Counted whether or not the stream delivers
    /// incomplete frames.
    pub frames_incomplete: u64,
    /// Blocks never seen at all: gaps in the block-id sequence.
    pub frames_missed: u64,
    /// Frames assembled and then discarded because the application did not
    /// take them before the delivery queue filled.
    pub frames_backpressure: u64,
    /// Blocks whose payload type the engine does not assemble.
    pub frames_unsupported: u64,
    /// Datagrams read from the socket. USB3 Vision: bulk transfers.
    pub packets_received: u64,
    /// Packets never received, of frames closed incomplete. A lower bound when
    /// a trailer is missing, since the trailer is what says how many there were.
    pub packets_missing: u64,
    /// Packets received more than once.
    pub packets_duplicate: u64,
    /// Packets received and not used, for any reason: not GVSP, a format the
    /// engine does not assemble, beyond the end of the frame, late, an error
    /// status, or from an address other than the camera's.
    pub packets_ignored: u64,
    /// Packets asked for again, each counted once however often it was asked.
    pub resend_requested: u64,
    /// Packets asked for again that then arrived.
    pub resend_recovered: u64,
    /// Packets asked for again that the camera answered it no longer has.
    pub resend_unavailable: u64,
    /// USB3 Vision: transfers that failed — a stall, a fault — and that the
    /// stream recovered from by resetting its endpoint, at the cost of the
    /// block in progress. 0 on GigE Vision.
    pub transfer_errors: u64,
    /// Payload bytes of complete frames handed to the application.
    pub bytes: u64,
    /// Smoothed payload throughput of complete frames, in megabits a second.
    pub avg_mbps: f64,
    /// Smoothed delay from the device timestamp to delivery, in milliseconds,
    /// when the stream can map device time to host time.
    pub avg_latency_ms: Option<f64>,
    /// Time since the stream started, in milliseconds.
    pub elapsed_ms: u64,
}

impl StreamCounters {
    /// Frames the application did not receive complete: incomplete, missed
    /// and dropped to backpressure.
    pub fn frames_lost(&self) -> u64 {
        self.frames_incomplete + self.frames_missed + self.frames_backpressure
    }
}

#[cfg(feature = "genicam")]
#[cfg_attr(docsrs, doc(cfg(feature = "genicam")))]
impl From<&viva_genicam::StreamStats> for StreamCounters {
    fn from(stats: &viva_genicam::StreamStats) -> Self {
        Self {
            frames_complete: stats.frames_complete,
            frames_incomplete: stats.frames_incomplete,
            frames_missed: stats.frames_missed,
            frames_backpressure: stats.frames_backpressure,
            frames_unsupported: stats.frames_unsupported,
            packets_received: stats.packets_received,
            packets_missing: stats.packets_missing,
            packets_duplicate: stats.packets_duplicate,
            packets_ignored: stats.packets_ignored(),
            resend_requested: stats.resend_requested,
            resend_recovered: stats.resend_recovered,
            resend_unavailable: stats.resend_unavailable,
            transfer_errors: stats.transfer_errors,
            bytes: stats.bytes,
            avg_mbps: stats.avg_mbps,
            avg_latency_ms: stats.avg_latency_ms,
            elapsed_ms: u64::try_from(stats.elapsed.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

// ── Image ────────────────────────────────────────────────────────────────────

/// [`ImageMeta::pixel_format`] on the wire: the format's PFNC name
/// (`"Mono8"`, `"RGB8Packed"`), or `"Unknown"` for a code PFNC does not name.
/// Reading accepts the older spellings cameras report (`"RGB8"`), as
/// [`viva_pfnc::PixelFormat::from_name`] does.
mod pfnc_name {
    use serde::{Deserialize, Deserializer, Serializer};
    use viva_pfnc::PixelFormat;

    pub(crate) fn serialize<S: Serializer>(
        format: &PixelFormat,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(format.name().unwrap_or("Unknown"))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PixelFormat, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(PixelFormat::from_name(&name))
    }
}

/// Image stream metadata published at acquisition start and on format change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct ImageMeta {
    /// The camera's pixel format; on the wire, its PFNC name. `viva-pfnc` is
    /// the one authority on formats (ADR-0030).
    #[serde(with = "pfnc_name")]
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub pixel_format: viva_pfnc::PixelFormat,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bytes a frame's payload carries: the camera's `PayloadSize`, padding
    /// and chunk data included.
    pub payload_size: u64,
}

impl ImageMeta {
    /// Image geometry and the bytes a frame carries.
    pub fn new(
        pixel_format: viva_pfnc::PixelFormat,
        width: u32,
        height: u32,
        payload_size: u64,
    ) -> Self {
        Self {
            pixel_format,
            width,
            height,
            payload_size,
        }
    }
}

// ── Announce cadence ─────────────────────────────────────────────────────────

/// How often a service re-announces each camera it serves.
///
/// Independent of how often the service *discovers* cameras: the GigE service
/// announced from inside its discovery loop, every discovery timeout plus
/// discovery interval (2 s + 5 s), which outlived [`ANNOUNCE_EXPIRY`] and made
/// cameras flicker in Viva Studio (backlog SVC-01).
pub const ANNOUNCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a client keeps a camera whose announces have stopped: three
/// missed [`ANNOUNCE_INTERVAL`]s.
pub const ANNOUNCE_EXPIRY: std::time::Duration = std::time::Duration::from_secs(6);

// ── Key Expressions ──────────────────────────────────────────────────────────

/// Key expression constants and helpers for the GenICam Zenoh API.
pub mod keys {
    /// Wildcard subscription for all device announcements.
    pub const ANNOUNCE_ALL: &str = "genicam/devices/*/announce";

    /// Where a camera is announced.
    pub fn announce(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/announce")
    }

    /// Where its XML is asked for.
    pub fn xml(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/xml")
    }

    /// Key expression for the diagnostic report queryable.
    /// Direction: App -> Service (queryable GET).
    pub fn report(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/report")
    }

    /// Where its connection status is published.
    pub fn status(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/status")
    }

    /// Where a feature's value is published.
    pub fn node_value(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/value")
    }

    /// Every feature value of a camera, for a subscriber.
    pub fn node_value_wildcard(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/*/value")
    }

    /// Where a feature is written.
    pub fn node_set(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/set")
    }

    /// Where a command is executed.
    pub fn node_execute(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/execute")
    }

    /// Key expression for a single-node [`super::FeatureState`] queryable.
    /// Direction: App -> Service (queryable GET). Reply payload is a
    /// [`super::FeatureState`] JSON object.
    pub fn node_introspect(device_id: &str, node_name: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/{node_name}/state")
    }

    /// Wildcard subscription for every node's [`super::FeatureState`]
    /// publication. Direction: Service -> App.
    pub fn node_state_wildcard(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/nodes/*/state")
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

    /// Where acquisition is started and stopped.
    pub fn acquisition_control(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/acquisition/control")
    }

    /// Where acquisition status is published.
    pub fn acquisition_status(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/acquisition/status")
    }

    /// Where image frames are published.
    pub fn image(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/image")
    }

    /// Where image metadata is published.
    pub fn image_meta(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/image/meta")
    }

    /// Key expression for event-vision blocks.
    /// Direction: Service -> subscribers (`put` per block). Payload is a
    /// [`super::EvsHeader`] followed by the encoded events.
    ///
    /// Event data never goes on [`image`]: it has no image geometry, and
    /// before API version 4 an event block was published there as a
    /// 1 × N "image" in a format clients decode as `Unknown`. Named `evs`,
    /// not `events`, so it cannot be mistaken for GenICam device events
    /// (GVCP `EVENT` messages such as `EventExposureEnd`).
    pub fn evs(device_id: &str) -> String {
        format!("genicam/devices/{device_id}/evs")
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

    /// A USB3 Vision camera is keyed by what it is — vendor, product,
    /// serial — not where it is plugged in, so moving it to another port
    /// keeps its id (UV-03); only a camera without a serial falls back to
    /// its bus position. Whatever the serial holds, the id is one key
    /// expression chunk.
    #[test]
    fn a_u3v_device_id_is_keyed_by_serial() {
        assert_eq!(
            u3v_device_id(0x164c, 0x5533, Some("FF003089"), "1", 3),
            "cam-u3v-164c5533-FF003089"
        );
        assert_eq!(
            u3v_device_id(0x164c, 0x5533, Some("FF003089"), "20", 9),
            u3v_device_id(0x164c, 0x5533, Some("FF003089"), "1", 3),
        );
        assert_eq!(
            u3v_device_id(0x2676, 0xba02, None, "2", 7),
            "cam-u3v-2676ba02-2-7"
        );
        assert_eq!(
            u3v_device_id(0x2676, 0xba02, Some("  "), "2", 7),
            "cam-u3v-2676ba02-2-7"
        );
        assert_eq!(
            u3v_device_id(0x2676, 0xba02, Some("a/b*c$#?"), "2", 7),
            "cam-u3v-2676ba02-a_2fb_2ac_24_23_3f"
        );
    }

    /// Codex review of #205: replacing each character a key cannot carry by
    /// `_` made `a/b` and `a*b` one id, and discovery skipped the second
    /// camera. The escape is reversible, so distinct serials stay distinct.
    #[test]
    fn serials_that_differ_only_where_a_key_cannot_carry_them_stay_distinct() {
        let id = |serial| u3v_device_id(0x2676, 0xba02, Some(serial), "2", 7);
        assert_ne!(id("a/b"), id("a*b"));
        assert_ne!(id("a_b"), id("a/b"));
        assert_ne!(id("a_2fb"), id("a/b"));
    }

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
    fn device_announce_round_trips_every_identity_field() {
        let mut a = DeviceAnnounce::new("cam-deadbeefcafe", "Left", "FakeGigE", "FAKE-001");
        a.ip = Some("192.168.1.10".into());
        a.mac = Some("DE:AD:BE:EF:CA:FE".into());
        a.user_name = Some("Left".into());
        a.manufacturer = Some("viva-genicam".into());
        let json = serde_json::to_string(&a).expect("serialize");
        let back: DeviceAnnounce = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, a);
        assert_eq!(back.api_version, Some(API_VERSION));
    }

    #[test]
    fn device_announce_omits_unset_identity_fields() {
        let a = DeviceAnnounce::new("cam-u3v-00010002", "U3V", "U3V", "");
        let json = serde_json::to_string(&a).expect("serialize");
        for field in ["ip", "mac", "user_name", "manufacturer"] {
            assert!(!json.contains(&format!("\"{field}\"")), "{field} in {json}");
        }
        let back: DeviceAnnounce = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, a);
    }

    /// What a version 2 service publishes: the device id in `serial` and none
    /// of the identity fields. It must still deserialise, or upgrading the app
    /// would make every older service's cameras disappear.
    #[test]
    fn device_announce_deserializes_a_version_2_payload() {
        let v2 = r#"{"id":"cam-deadbeefcafe","name":"FakeGigE","model":"FakeGigE","serial":"cam-deadbeefcafe","api_version":2}"#;
        let a: DeviceAnnounce = serde_json::from_str(v2).expect("should deserialize");
        assert_eq!(a.api_version, Some(2));
        assert_eq!(a.serial, "cam-deadbeefcafe");
        assert_eq!(a.ip, None);
        assert_eq!(a.mac, None);
        assert_eq!(a.user_name, None);
        assert_eq!(a.manufacturer, None);
    }

    #[test]
    fn for_gige_carries_the_device_identity() {
        let a = DeviceAnnounce::for_gige(
            &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
            std::net::Ipv4Addr::new(192, 168, 1, 10),
            Some("FakeGigE"),
            Some("FAKE-001"),
            Some("Left"),
            Some("viva-genicam"),
        );
        assert_eq!(a.id, "cam-deadbeefcafe");
        assert_eq!(a.name, "Left");
        assert_eq!(a.model, "FakeGigE");
        assert_eq!(a.serial, "FAKE-001");
        assert_eq!(a.ip.as_deref(), Some("192.168.1.10"));
        assert_eq!(a.mac.as_deref(), Some("DE:AD:BE:EF:CA:FE"));
        assert_eq!(a.user_name.as_deref(), Some("Left"));
        assert_eq!(a.manufacturer.as_deref(), Some("viva-genicam"));
        assert_eq!(a.api_version, Some(API_VERSION));
    }

    #[test]
    fn for_gige_names_by_user_name_then_model_then_address() {
        let mac = [0, 1, 2, 3, 4, 5];
        let ip = std::net::Ipv4Addr::new(10, 0, 0, 7);
        let name = |model, user_name| {
            DeviceAnnounce::for_gige(&mac, ip, model, None, user_name, None).name
        };
        assert_eq!(name(Some("M"), Some("U")), "U");
        assert_eq!(name(Some("M"), None), "M");
        assert_eq!(name(None, None), "10.0.0.7");
    }

    /// An absent serial stays empty: services before version 3 filled it with
    /// the device id, which is not a serial and read as one.
    #[test]
    fn for_gige_does_not_invent_a_serial() {
        let a = DeviceAnnounce::for_gige(
            &[0, 1, 2, 3, 4, 5],
            std::net::Ipv4Addr::new(10, 0, 0, 7),
            Some("M"),
            None,
            None,
            None,
        );
        assert_eq!(a.serial, "");
        assert_eq!(a.user_name, None);
        assert_eq!(a.manufacturer, None);
    }

    #[test]
    fn gige_device_id_is_lowercase_mac_hex() {
        assert_eq!(
            gige_device_id(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]),
            "cam-deadbeefcafe"
        );
        assert_eq!(
            gige_device_id(&[0x00, 0x0C, 0xDF, 0x06, 0x5B, 0x2F]),
            "cam-000cdf065b2f"
        );
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

    /// GA-05: the wire state says how to refresh a feature — its polling time
    /// and its caching mode, both resolved along its value path — so a remote
    /// viewer polls what a local one does.
    #[cfg(feature = "genapi")]
    #[test]
    fn a_genapi_state_carries_its_polling_time_and_caching_mode() {
        let xml = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Float Name="DeviceTemperature"><pValue>DeviceTemperatureReg</pValue></Float>
            <FloatReg Name="DeviceTemperatureReg"><Address>0x0</Address><Length>4</Length><AccessMode>RO</AccessMode><Cachable>NoCache</Cachable><PollingTime>500</PollingTime></FloatReg>
            <Integer Name="Width"><Address>0x4</Address><Length>4</Length><AccessMode>RW</AccessMode></Integer>
        </RegisterDescription>"#;
        let model = viva_genapi_xml::parse(xml).expect("parse");
        let map = viva_genapi::NodeMap::try_from_xml(model).expect("build");
        let io = viva_genapi::NullIo;
        let wire = FeatureState::from(map.feature_state("DeviceTemperature", &io).unwrap());
        assert_eq!(wire.polling_ms, Some(500));
        assert_eq!(wire.caching.as_deref(), Some("NoCache"));
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains(r#""polling_ms":500"#), "{json}");
        let wire = FeatureState::from(map.feature_state("Width", &io).unwrap());
        assert_eq!(wire.polling_ms, None);
        assert_eq!(wire.caching.as_deref(), Some("WriteThrough"));
    }

    // ── AcquisitionStatus ───────────────────────────────────────────────────

    /// The status carries the stream's counters, and a stopped stream's final
    /// ones, so a client can show more than one lost-frame count.
    #[test]
    fn an_acquisition_status_round_trips_its_counters() {
        let stats = StreamCounters {
            frames_complete: 2_410,
            frames_incomplete: 2,
            frames_missed: 1,
            packets_received: 1_000_000,
            transfer_errors: 3,
            avg_mbps: 812.5,
            avg_latency_ms: Some(1.25),
            elapsed_ms: 10_000,
            ..Default::default()
        };

        for status in [
            AcquisitionStatus::active(Some(241.0), Some(stats)),
            AcquisitionStatus::stopped(stats),
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: AcquisitionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status, "{json}");
        }
        assert_eq!(stats.frames_lost(), 3);
        let stopped = AcquisitionStatus::stopped(stats);
        assert!(!stopped.active);
        assert_eq!(stopped.fps, None);

        // Nothing to count says so, rather than a row of zeros.
        let json = serde_json::to_value(AcquisitionStatus::inactive()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "active": false, "fps": null, "stats": null })
        );
    }

    /// The six reasons a packet goes unused are one count on the wire, and
    /// the elapsed time is whole milliseconds.
    #[cfg(feature = "genicam")]
    #[test]
    fn stream_counters_carry_the_librarys_stream_stats() {
        let mut stats = viva_genicam::StreamStats::default();
        stats.frames_complete = 10;
        stats.frames_incomplete = 1;
        stats.frames_missed = 2;
        stats.frames_backpressure = 3;
        stats.frames_unsupported = 4;
        stats.packets_received = 500;
        stats.packets_missing = 5;
        stats.packets_duplicate = 6;
        stats.packets_ignored_malformed = 1;
        stats.packets_ignored_unsupported = 2;
        stats.packets_ignored_out_of_range = 3;
        stats.packets_ignored_late = 4;
        stats.packets_ignored_error_status = 5;
        stats.packets_ignored_foreign = 6;
        stats.resend_requested = 7;
        stats.resend_recovered = 8;
        stats.resend_unavailable = 9;
        stats.transfer_errors = 11;
        stats.bytes = 12_345;
        stats.avg_mbps = 98.5;
        stats.avg_latency_ms = Some(0.5);
        stats.elapsed = std::time::Duration::from_micros(2_500_900);

        let wire = StreamCounters::from(&stats);
        assert_eq!(wire.packets_ignored, 21);
        assert_eq!(wire.packets_ignored, stats.packets_ignored());
        assert_eq!(wire.elapsed_ms, 2_500);
        assert_eq!(wire.frames_lost(), stats.frames_lost());
        assert_eq!(
            (
                wire.frames_complete,
                wire.frames_incomplete,
                wire.frames_missed,
                wire.frames_backpressure,
                wire.frames_unsupported,
            ),
            (10, 1, 2, 3, 4)
        );
        assert_eq!(
            (
                wire.packets_received,
                wire.packets_missing,
                wire.packets_duplicate,
            ),
            (500, 5, 6)
        );
        assert_eq!(
            (
                wire.resend_requested,
                wire.resend_recovered,
                wire.resend_unavailable,
                wire.transfer_errors,
            ),
            (7, 8, 9, 11)
        );
        assert_eq!(wire.bytes, 12_345);
        assert_eq!(wire.avg_mbps, 98.5);
        assert_eq!(wire.avg_latency_ms, Some(0.5));
    }

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
            polling_ms: None,
            caching: None,
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
            polling_ms: None,
            caching: None,
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
            polling_ms: None,
            caching: None,
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
            polling_ms: None,
            caching: None,
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
                polling_ms: None,
                caching: None,
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

    /// `ImageMeta` names the camera's format as PFNC does, every format PFNC
    /// names: the wire's own enum named 54 and sent the rest as `"Unknown"`
    /// (backlog API-10).
    #[test]
    fn image_meta_names_the_format_as_pfnc_does() {
        for (format, name) in [
            (viva_pfnc::PixelFormat::Mono8, "Mono8"),
            (viva_pfnc::PixelFormat::RGB8Packed, "RGB8Packed"),
            (viva_pfnc::PixelFormat::Coord3DC16, "Coord3D_C16"),
            (viva_pfnc::PixelFormat::Coord3DABC32f, "Coord3D_ABC32f"),
            (viva_pfnc::PixelFormat::from_code(0x0123_4567), "Unknown"),
        ] {
            let meta = ImageMeta::new(format, 4, 2, 8);
            let json = serde_json::to_value(&meta).expect("serialize");
            assert_eq!(json["pixel_format"], name, "{format}");
        }
    }

    /// A client reads the name back, the older spellings cameras use too.
    #[test]
    fn image_meta_reads_the_pfnc_name_and_its_older_spellings() {
        for (name, format) in [
            ("Mono12Packed", viva_pfnc::PixelFormat::Mono12Packed),
            ("RGB8", viva_pfnc::PixelFormat::RGB8Packed),
            ("YUV422Packed", viva_pfnc::PixelFormat::YUV422_8_UYVY),
            ("Fancy42", viva_pfnc::PixelFormat::Unknown(0)),
        ] {
            let json =
                format!(r#"{{"pixel_format":"{name}","width":1,"height":1,"payload_size":1}}"#);
            let meta: ImageMeta = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(meta.pixel_format, format, "{name}");
        }
    }
}
