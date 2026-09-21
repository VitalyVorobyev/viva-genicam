#![cfg_attr(docsrs, feature(doc_cfg))]
//! Load and pre-parse GenICam XML using quick-xml.
//!
//! This crate provides types and functions for parsing GenICam XML descriptions
//! into a structured representation that can be used by the core evaluation engine.

mod builders;
#[cfg(feature = "fetch")]
mod fetch;
mod parsers;
mod util;

#[cfg(feature = "fetch")]
pub use fetch::fetch_and_load_xml;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use parsers::{
    parse_boolean, parse_category, parse_category_empty, parse_command, parse_command_empty,
    parse_converter, parse_enum, parse_float, parse_int_converter, parse_integer, parse_register,
    parse_string, parse_struct_reg, parse_swissknife,
};
use util::{attribute_value, skip_element};

/// Source of the numeric value backing an enumeration entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnumValueSrc {
    /// Numeric literal declared directly in the XML.
    Literal(i64),
    /// Value obtained from another node referenced via `<pValue>`.
    FromNode(String),
}

/// References to predicate provider nodes used for runtime gating.
///
/// GenICam's `pIsImplemented`, `pIsAvailable` and `pIsLocked` each point at
/// another node (typically an Integer, Boolean or IntSwissKnife) whose current
/// value gates whether a feature is implemented, accessible, or writable.
/// These three references are shared by most node variants, so we collect them
/// into one struct to keep the variant fields small.
///
/// All three fields are optional; a node with no predicates has
/// [`PredicateRefs::default()`]. Serde fields use the GenICam XML spelling so
/// round-trip JSON matches the XML attribute names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicateRefs {
    /// Name of a node evaluating to non-zero iff the feature is implemented.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pIsImplemented"
    )]
    pub p_is_implemented: Option<String>,
    /// Name of a node evaluating to non-zero iff the feature is accessible now.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pIsAvailable"
    )]
    pub p_is_available: Option<String>,
    /// Name of a node evaluating to non-zero iff the feature is locked (RW→RO).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "pIsLocked")]
    pub p_is_locked: Option<String>,
}

impl PredicateRefs {
    /// Iterate over all referenced node names (for dependency-graph walks).
    pub fn references(&self) -> impl Iterator<Item = &str> {
        [
            self.p_is_implemented.as_deref(),
            self.p_is_available.as_deref(),
            self.p_is_locked.as_deref(),
        ]
        .into_iter()
        .flatten()
    }

    /// `true` when every field is `None`.
    pub fn is_empty(&self) -> bool {
        self.p_is_implemented.is_none()
            && self.p_is_available.is_none()
            && self.p_is_locked.is_none()
    }
}

/// Declaration for a single enumeration entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumEntryDecl {
    /// Symbolic entry name exposed to clients.
    pub name: String,
    /// Source describing how to resolve the numeric value for this entry.
    pub value: EnumValueSrc,
    /// Optional user facing label.
    pub display_name: Option<String>,
    /// Predicate refs controlling whether this entry is implemented / available.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum XmlError {
    #[error("xml: {0}")]
    Xml(String),
    #[error("invalid descriptor: {0}")]
    Invalid(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("unsupported URL: {0}")]
    Unsupported(String),
}

/// Visibility level controlling which users see a feature.
///
/// GenICam defines four levels; features at a given level are visible to
/// users at that level and above.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[non_exhaustive]
pub enum Visibility {
    /// Shown to all users (default).
    #[default]
    Beginner,
    /// Shown to experienced users.
    Expert,
    /// Shown only to advanced integrators.
    Guru,
    /// Hidden from all UI presentations.
    Invisible,
}

impl Visibility {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Beginner" => Some(Self::Beginner),
            "Expert" => Some(Self::Expert),
            "Guru" => Some(Self::Guru),
            "Invisible" => Some(Self::Invisible),
            _ => None,
        }
    }
}

/// Recommended UI representation for a numeric feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Representation {
    Linear,
    Logarithmic,
    Boolean,
    PureNumber,
    HexNumber,
    /// Display as dotted-quad IPv4 address.
    IPV4Address,
    /// Display as colon-separated MAC address.
    MACAddress,
}

impl Representation {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Linear" => Some(Self::Linear),
            "Logarithmic" => Some(Self::Logarithmic),
            "Boolean" => Some(Self::Boolean),
            "PureNumber" => Some(Self::PureNumber),
            "HexNumber" => Some(Self::HexNumber),
            "IPV4Address" => Some(Self::IPV4Address),
            "MACAddress" => Some(Self::MACAddress),
            _ => None,
        }
    }
}

/// Shared metadata present on every GenICam node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMeta {
    /// Visibility level (Beginner, Expert, Guru, Invisible).
    pub visibility: Visibility,
    /// Long-form description of the feature.
    pub description: Option<String>,
    /// Short tooltip text for UI hover hints.
    pub tooltip: Option<String>,
    /// Human-readable label (may differ from the node name).
    pub display_name: Option<String>,
    /// Recommended UI representation for numeric features.
    pub representation: Option<Representation>,
}

/// Access privileges for a GenICam node as described in the XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessMode {
    /// Read-only node. The underlying register must not be modified by the client.
    RO,
    /// Write-only node. Reading the register is not permitted.
    WO,
    /// Read-write node. The register may be read and written by the client.
    RW,
}

impl AccessMode {
    /// Parse an `<AccessMode>` value.
    ///
    /// Beyond the three spellings the standard defines, the single-letter forms
    /// `R` / `W` are accepted: they appear in third-party GenICam documents
    /// (including the standard's own conformance fixtures). An unrecognized
    /// value falls back to `RW` — the same default as an absent `<AccessMode>` —
    /// so one odd register cannot make a whole camera unusable. The runtime
    /// still surfaces the device's own error if the access is genuinely denied.
    pub(crate) fn parse(value: &str) -> Result<Self, XmlError> {
        let trimmed = value.trim();
        match trimmed.to_ascii_uppercase().as_str() {
            "RO" | "R" => Ok(AccessMode::RO),
            "WO" | "W" => Ok(AccessMode::WO),
            "RW" | "WR" => Ok(AccessMode::RW),
            other => {
                tracing::warn!(
                    access_mode = other,
                    "unknown <AccessMode> value, assuming RW"
                );
                Ok(AccessMode::RW)
            }
        }
    }
}

/// Register addressing metadata for a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Addressing {
    /// Register address is the **sum** of every declared address term.
    ///
    /// This is the GenICam register address model: `<Address>`, `<pAddress>`
    /// and `<pIndex>` may each appear any number of times on one register and
    /// all of them add up. A node such as
    ///
    /// ```xml
    /// <IntReg Name="SerialPortSource_Val">
    ///   <pAddress>SerialPortSourceAddr</pAddress>
    ///   <Address>0x00000008</Address>
    /// </IntReg>
    /// ```
    ///
    /// lives eight bytes past the block base, not at the base and not at
    /// offset eight. Keeping only one term reads the wrong register and says
    /// nothing about it — that was issue #35.
    Sum {
        /// Terms to add together, in declaration order.
        terms: Vec<AddressTerm>,
        /// Length of the register block in bytes.
        len: u32,
    },
    /// Node switches between register blocks based on a selector value.
    ///
    /// This is `<Selected>`/`<pSelected>`, which picks one block rather than
    /// contributing an offset, so it is not a term in the sum above.
    BySelector {
        /// Name of the selector node controlling the address.
        selector: String,
        /// Mapping of selector value to `(address, length)` pair.
        map: Vec<(String, (u64, u32))>,
    },
}

/// One contribution to a register address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddressTerm {
    /// `<Address>` — a literal offset.
    Fixed(u64),
    /// `<pAddress>` — an offset read from another node at runtime.
    Node(String),
    /// `<pIndex>` — an index read from another node, scaled by a stride.
    Index {
        /// Node supplying the index.
        node: String,
        /// Stride each index step advances the address by.
        offset: IndexOffset,
    },
}

/// Stride applied to a `<pIndex>` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexOffset {
    /// `<pIndex Offset="64">` — a literal stride.
    Fixed(u64),
    /// `<pIndex pOffset="Node">` — a stride read from another node.
    Node(String),
    /// `<pIndex>` with neither attribute: the stride is the register length.
    Length,
}

impl Addressing {
    /// A plain register at a literal address.
    pub fn fixed(address: u64, len: u32) -> Self {
        Addressing::Sum {
            terms: vec![AddressTerm::Fixed(address)],
            len,
        }
    }

    /// Register block length in bytes, when it does not depend on a selector.
    pub fn byte_len(&self) -> Option<u32> {
        match self {
            Addressing::Sum { len, .. } => Some(*len),
            Addressing::BySelector { .. } => None,
        }
    }

    /// Names of every node this addressing reads at resolution time.
    ///
    /// Used to build the invalidation graph: when one of these changes, the
    /// address changes, so anything cached against it is stale.
    pub fn referenced_nodes(&self) -> Vec<&str> {
        match self {
            Addressing::Sum { terms, .. } => terms
                .iter()
                .flat_map(|term| match term {
                    AddressTerm::Fixed(_) => Vec::new(),
                    AddressTerm::Node(node) => vec![node.as_str()],
                    AddressTerm::Index { node, offset } => match offset {
                        IndexOffset::Node(stride) => vec![node.as_str(), stride.as_str()],
                        _ => vec![node.as_str()],
                    },
                })
                .collect(),
            Addressing::BySelector { selector, .. } => vec![selector.as_str()],
        }
    }
}

/// Whether a register's bits are interpreted as a signed or unsigned integer.
///
/// GenICam's default is `Unsigned`; `<Sign>Signed</Sign>` opts in. Getting
/// this wrong is invisible until a register's top bit is set — a
/// `GevCurrentIPAddress` of 192.168.1.160 then reads as a large negative
/// number, and a mask comparison such as `(CTRL | 0xFDFFFFFF) = 0xFFFFFFFF`
/// can never be true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sign {
    /// `<Sign>Unsigned</Sign>`, and the default when `<Sign>` is absent.
    #[default]
    Unsigned,
    /// `<Sign>Signed</Sign>`: the payload is two's complement.
    Signed,
}

impl Sign {
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "signed" => Some(Sign::Signed),
            "unsigned" => Some(Sign::Unsigned),
            _ => None,
        }
    }

    /// Whether values should be sign-extended.
    pub fn is_signed(self) -> bool {
        matches!(self, Sign::Signed)
    }
}

/// Byte order used to interpret a multi-byte register payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ByteOrder {
    /// The first byte contains the least significant bits.
    Little,
    /// The first byte contains the most significant bits.
    Big,
}

impl ByteOrder {
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "littleendian" => Some(ByteOrder::Little),
            "bigendian" => Some(ByteOrder::Big),
            _ => None,
        }
    }
}

fn default_big_endian() -> ByteOrder {
    ByteOrder::Big
}

/// Bitfield metadata describing a sub-range of a register payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitField {
    /// Starting bit offset within the interpreted register value.
    pub bit_offset: u16,
    /// Number of bits covered by the field.
    pub bit_length: u16,
    /// Byte order used when interpreting the enclosing register.
    pub byte_order: ByteOrder,
}

/// Byte-level encoding of the payload behind a `<Float>` / `<FloatReg>` node.
///
/// GenICam's XSD lets a float feature be backed by either:
///
/// - a native IEEE 754 register — a `<FloatReg>` element, or a `<Float>` whose
///   addressing reads exactly 4 or 8 bytes and carries no `<Scale>`/`<Offset>`;
/// - a scaled integer register — a `<Float>` that declares `<Scale>` and/or
///   `<Offset>` and reads the register bytes as a signed integer.
///
/// Prior to this field, `get_float`/`set_float` always used the scaled-integer
/// codec. That returned the bit pattern of the IEEE 754 value decoded as i64
/// for fields such as `AcquisitionFrameRate` (e.g. `1106247680` for 30.0) —
/// see `doc/2026-04-12-genapi-numeric-type-dispatch.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FloatEncoding {
    /// Register bytes are an IEEE 754 value (4 bytes → f32, 8 bytes → f64).
    Ieee754,
    /// Register bytes are a signed integer; `Scale` and `Offset` map to the
    /// user-facing value.
    #[default]
    ScaledInteger,
}

/// Output type of a SwissKnife expression node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SkOutput {
    /// Integer output. The runtime rounds the computed value to the nearest
    /// integer with ties going towards zero.
    Integer,
    /// Floating point output. The runtime exposes the value as a `f64` without
    /// any additional processing.
    #[default]
    Float,
}

impl SkOutput {
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "integer" => Some(SkOutput::Integer),
            "float" => Some(SkOutput::Float),
            _ => None,
        }
    }
}
/// Named literals and sub-formulas declared alongside a formula.
///
/// GenApi lets a `<SwissKnife>`, `<IntSwissKnife>` or `<Converter>` name parts
/// of its own formula:
///
/// ```xml
/// <Constant Name="TEN">10</Constant>
/// <Expression Name="XPLUS2">TEN + X</Expression>
/// <Formula>TEN * XPLUS2</Formula>
/// ```
///
/// Both are resolved by substitution when the runtime builds the node, so the
/// evaluator never sees them. Declaration order matters: an `<Expression>` may
/// refer to constants and to expressions declared before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormulaBindings {
    /// `<Constant Name="..">literal</Constant>` pairs, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<(String, String)>,
    /// `<Expression Name="..">formula</Expression>` pairs, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expressions: Vec<(String, String)>,
}

impl FormulaBindings {
    /// Whether nothing was declared.
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty() && self.expressions.is_empty()
    }
}

/// Declaration of a SwissKnife node consisting of an arithmetic expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwissKnifeDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Shared metadata.
    pub meta: NodeMeta,
    /// Raw expression string to be parsed by the runtime.
    pub expr: String,
    /// Mapping of variables used in the expression to provider node names.
    pub variables: Vec<(String, String)>,
    /// Desired output type (integer or float).
    pub output: SkOutput,
    /// Named literals and sub-formulas referenced by `expr`.
    #[serde(default, skip_serializing_if = "FormulaBindings::is_empty")]
    pub bindings: FormulaBindings,
    /// Predicate refs gating implementation / availability.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

/// Declaration of a Converter node for bidirectional value transformation.
///
/// Converters expose a floating-point value computed from an underlying
/// register or node via a formula.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConverterDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Shared metadata.
    pub meta: NodeMeta,
    /// Name of the node providing the raw register value.
    pub p_value: String,
    /// `<FormulaTo>`: converts the feature value to the raw register value.
    /// This is the *write* direction; its `FROM` variable is the incoming
    /// value.
    pub formula_to: String,
    /// `<FormulaFrom>`: converts the raw register value to the feature value.
    /// This is the *read* direction; its `TO` variable is the raw value.
    pub formula_from: String,
    /// Mapping of formula variables to provider node names for `formula_to`.
    pub variables_to: Vec<(String, String)>,
    /// Mapping of formula variables to provider node names for `formula_from`.
    pub variables_from: Vec<(String, String)>,
    /// Named literals and sub-formulas referenced by either formula.
    #[serde(default, skip_serializing_if = "FormulaBindings::is_empty")]
    pub bindings: FormulaBindings,
    /// Engineering unit (if provided).
    pub unit: Option<String>,
    /// Desired output type.
    pub output: SkOutput,
    /// Predicate refs gating implementation / availability / lock state.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

/// Declaration of an IntConverter node for integer-specific bidirectional conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntConverterDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Shared metadata.
    pub meta: NodeMeta,
    /// Name of the node providing the raw register value.
    pub p_value: String,
    /// `<FormulaTo>`: converts the feature value to the raw register value.
    /// This is the *write* direction; its `FROM` variable is the incoming
    /// value.
    pub formula_to: String,
    /// `<FormulaFrom>`: converts the raw register value to the feature value.
    /// This is the *read* direction; its `TO` variable is the raw value.
    pub formula_from: String,
    /// Mapping of formula variables to provider node names for `formula_to`.
    pub variables_to: Vec<(String, String)>,
    /// Mapping of formula variables to provider node names for `formula_from`.
    pub variables_from: Vec<(String, String)>,
    /// Named literals and sub-formulas referenced by either formula.
    #[serde(default, skip_serializing_if = "FormulaBindings::is_empty")]
    pub bindings: FormulaBindings,
    /// Engineering unit (if provided).
    pub unit: Option<String>,
    /// Predicate refs gating implementation / availability / lock state.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

/// Declaration of a StringReg node for string-typed register access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Shared metadata.
    pub meta: NodeMeta,
    /// Addressing metadata for the register block.
    pub addressing: Addressing,
    /// Access privileges.
    pub access: AccessMode,
    /// Predicate refs gating implementation / availability / lock state.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

/// Declaration of a `<Register>` node — raw byte-array register access.
///
/// The base register type: an address, a byte count, and no interpretation of
/// the bytes at all. `<StringReg>` is this plus UTF-8/NUL decoding.
///
/// The length lives in [`Addressing`] rather than in a field here, because a
/// bare `<pIndex>` strides by the register's length and address resolution
/// already reads it from there. When `<pLength>` lands (GA-09 phase two) it
/// gains a length that overrides that value *after* the address resolves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Shared metadata.
    pub meta: NodeMeta,
    /// Addressing metadata for the register block, including its byte length.
    pub addressing: Addressing,
    /// Access privileges. An absent `<AccessMode>` means read-only.
    pub access: AccessMode,
    /// `<pPort>` target, verbatim; `None` when the element was absent.
    ///
    /// Anything other than `None` or `"Device"` is not routed yet — see GA-12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// Predicate refs gating implementation / availability / lock state.
    #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
    pub predicates: PredicateRefs,
}

/// Declaration of a node extracted from the GenICam XML description.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NodeDecl {
    /// Integer feature backed by a register block or delegated via pValue.
    Integer {
        /// Feature name.
        name: String,
        /// Shared metadata (visibility, description, tooltip, etc.).
        meta: NodeMeta,
        /// Addressing metadata (absent when delegated via `pvalue`).
        addressing: Option<Addressing>,
        /// Length in bytes of the register payload.
        len: u32,
        /// Access privileges.
        access: AccessMode,
        /// Minimum allowed user value.
        min: i64,
        /// Maximum allowed user value.
        max: i64,
        /// Optional increment step enforced by the device.
        inc: Option<i64>,
        /// Engineering unit (if provided).
        unit: Option<String>,
        /// Optional bitfield metadata describing the active bit range.
        bitfield: Option<BitField>,
        /// Whether the register payload is signed. Defaults to unsigned.
        #[serde(default)]
        sign: Sign,
        /// Selector nodes referencing this feature.
        selectors: Vec<String>,
        /// Selector gating rules in the form (selector name, allowed values).
        selected_if: Vec<(String, Vec<String>)>,
        /// Node providing the value (delegates read/write to another node).
        pvalue: Option<String>,
        /// Node providing the dynamic maximum.
        p_max: Option<String>,
        /// Node providing the dynamic minimum.
        p_min: Option<String>,
        /// Static value (for constant integer nodes with `<Value>`).
        value: Option<i64>,
        /// Predicate refs gating implementation / availability / lock state.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Floating point feature backed by an integer register with scaling,
    /// a native IEEE 754 register, or delegated via pValue.
    Float {
        name: String,
        meta: NodeMeta,
        /// Addressing metadata (absent when delegated via `pvalue`).
        addressing: Option<Addressing>,
        access: AccessMode,
        min: f64,
        max: f64,
        unit: Option<String>,
        /// Optional rational scale applied to the raw register value.
        scale: Option<(i64, i64)>,
        /// Optional additive offset applied after scaling.
        offset: Option<f64>,
        selectors: Vec<String>,
        selected_if: Vec<(String, Vec<String>)>,
        /// Node providing the value (delegates read/write to another node).
        pvalue: Option<String>,
        /// How the register payload should be interpreted — native IEEE 754
        /// or scaled integer. Defaults to [`FloatEncoding::ScaledInteger`] to
        /// preserve existing behaviour for XML that relied on it.
        #[serde(default)]
        encoding: FloatEncoding,
        /// Byte order of the register payload. Defaults to [`ByteOrder::Big`]
        /// (the GenICam default).
        #[serde(default = "default_big_endian")]
        byte_order: ByteOrder,
        /// Predicate refs gating implementation / availability / lock state.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Enumeration feature exposing a list of named integer values.
    Enum {
        name: String,
        meta: NodeMeta,
        /// Addressing metadata (absent when delegated via `pvalue`).
        addressing: Option<Addressing>,
        access: AccessMode,
        entries: Vec<EnumEntryDecl>,
        default: Option<String>,
        selectors: Vec<String>,
        selected_if: Vec<(String, Vec<String>)>,
        /// Node providing the integer value (delegates register read/write).
        pvalue: Option<String>,
        /// Predicate refs gating implementation / availability / lock state.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Boolean feature backed by a single bit/byte register or delegated via pValue.
    Boolean {
        name: String,
        meta: NodeMeta,
        /// Addressing metadata (absent when delegated via `pvalue`).
        addressing: Option<Addressing>,
        len: u32,
        access: AccessMode,
        bitfield: Option<BitField>,
        selectors: Vec<String>,
        selected_if: Vec<(String, Vec<String>)>,
        /// Node providing the value (delegates read/write to another node).
        pvalue: Option<String>,
        /// On value for pValue-backed booleans.
        on_value: Option<i64>,
        /// Off value for pValue-backed booleans.
        off_value: Option<i64>,
        /// Predicate refs gating implementation / availability / lock state.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Command feature that triggers an action when written.
    Command {
        name: String,
        meta: NodeMeta,
        /// Fixed register address (absent when delegated via `pvalue`).
        address: Option<u64>,
        len: u32,
        /// Node providing the command register (delegates write).
        pvalue: Option<String>,
        /// Value to write when executing the command.
        command_value: Option<i64>,
        /// Predicate refs gating implementation / availability / lock state.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Category used to organise features.
    Category {
        name: String,
        meta: NodeMeta,
        children: Vec<String>,
        /// Predicate refs gating implementation / availability.
        #[serde(default, skip_serializing_if = "PredicateRefs::is_empty")]
        predicates: PredicateRefs,
    },
    /// Computed value backed by an arithmetic expression referencing other nodes.
    SwissKnife(SwissKnifeDecl),
    /// Converter transforming raw values to/from user-facing floating-point values.
    Converter(ConverterDecl),
    /// IntConverter transforming raw values to/from user-facing integer values.
    IntConverter(IntConverterDecl),
    /// StringReg for string-typed register access.
    String(StringDecl),
    /// Raw byte-array register access.
    Register(RegisterDecl),
}

impl NodeDecl {
    /// Feature name declared by this node.
    pub fn name(&self) -> &str {
        match self {
            NodeDecl::Integer { name, .. }
            | NodeDecl::Float { name, .. }
            | NodeDecl::Enum { name, .. }
            | NodeDecl::Boolean { name, .. }
            | NodeDecl::Command { name, .. }
            | NodeDecl::Category { name, .. } => name,
            NodeDecl::SwissKnife(decl) => &decl.name,
            NodeDecl::Converter(decl) => &decl.name,
            NodeDecl::IntConverter(decl) => &decl.name,
            NodeDecl::String(decl) => &decl.name,
            NodeDecl::Register(decl) => &decl.name,
        }
    }

    /// Node kind, matching the variant name.
    ///
    /// This is the declaration kind rather than the originating XML tag: an
    /// `<IntReg>` and a `<MaskedIntReg>` both report `Integer`. Useful for
    /// grouping and for reporting nodes that were dropped downstream of
    /// parsing.
    pub fn kind(&self) -> &'static str {
        match self {
            NodeDecl::Integer { .. } => "Integer",
            NodeDecl::Float { .. } => "Float",
            NodeDecl::Enum { .. } => "Enum",
            NodeDecl::Boolean { .. } => "Boolean",
            NodeDecl::Command { .. } => "Command",
            NodeDecl::Category { .. } => "Category",
            NodeDecl::SwissKnife(_) => "SwissKnife",
            NodeDecl::Converter(_) => "Converter",
            NodeDecl::IntConverter(_) => "IntConverter",
            NodeDecl::String(_) => "String",
            NodeDecl::Register(_) => "Register",
        }
    }
}

/// A node element that could not be parsed and was left out of the model.
///
/// Parsing isolates each node, so a declaration we cannot make sense of costs
/// that one feature instead of the whole document. These records exist so the
/// loss is visible rather than silent — surface them in a UI, log them, or
/// assert on them in tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedNode {
    /// XML tag of the node element, e.g. `Integer`.
    pub tag: String,
    /// `Name` attribute, when the element had one.
    pub name: Option<String>,
    /// Rendered parse error explaining why the node was dropped.
    pub error: String,
}

/// Full XML model describing the GenICam schema version and all declared nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmlModel {
    /// Combined schema version extracted from the RegisterDescription attributes.
    pub version: String,
    /// Flat list of node declarations present in the document.
    pub nodes: Vec<NodeDecl>,
    /// Node elements that failed to parse and were skipped. Empty for a clean
    /// document.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedNode>,
}

/// Minimal metadata extracted from a quick XML scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimalXmlInfo {
    pub schema_version: Option<String>,
    pub top_level_features: Vec<String>,
}

/// Drop a leading UTF-8 byte-order mark.
///
/// A BOM is valid UTF-8 (`U+FEFF`), so it survives `String::from_utf8` and
/// reaches the parser intact; The Imaging Source's DMK 33GP2000e ships one
/// (issue #122). quick-xml removes it from its own view of the input but does
/// **not** advance `Reader::buffer_position`, so every offset it reports is
/// three bytes short of the true offset into `xml`. [`parse`] slices node
/// elements out of `xml` by those offsets, so each slice lost its closing `>`
/// and every node in the document was skipped. Stripping here keeps quick-xml's
/// positions and our `&str` talking about the same bytes.
fn strip_bom(xml: &str) -> &str {
    xml.strip_prefix('\u{feff}').unwrap_or(xml)
}

/// Parse a GenICam XML snippet and collect minimal metadata.
pub fn parse_into_minimal_nodes(xml: &str) -> Result<MinimalXmlInfo, XmlError> {
    let xml = strip_bom(xml);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Vendor XML is not ours to fix: a lone `&` in a tooltip must not stop us
    // from reading the document.
    reader.config_mut().allow_dangling_amp = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut schema_version: Option<String> = None;
    let mut top_level_features = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                handle_start(&e, depth, &mut schema_version, &mut top_level_features)?;
            }
            Ok(Event::Empty(e)) => {
                depth += 1;
                handle_start(&e, depth, &mut schema_version, &mut top_level_features)?;
                depth = depth.saturating_sub(1);
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(MinimalXmlInfo {
        schema_version,
        top_level_features,
    })
}

/// `true` for tags this parser turns into one or more [`NodeDecl`]s.
fn is_node_tag(tag: &str) -> bool {
    matches!(
        tag,
        "Integer"
            | "IntReg"
            | "MaskedIntReg"
            | "IntSwissKnife"
            | "SwissKnife"
            | "Float"
            | "FloatReg"
            | "Enumeration"
            | "Boolean"
            | "Command"
            | "Category"
            | "Converter"
            | "IntConverter"
            | "Register"
            | "StringReg"
            | "String"
            | "StructReg"
    )
}

/// Record an element this parser has no node type for, or `None` if it is not
/// a node at all.
///
/// A node declaration is told from a structural element by its `Name`
/// attribute, which the GenApi schema requires on every node and on nothing
/// else at this level. Without this, an unlisted tag fell through to
/// `skip_element` and vanished — no log line, no [`XmlModel::skipped`] entry,
/// nothing for a corpus test to trip over. `<Register>`, 56 declarations
/// across 14 corpus documents, disappeared exactly this way.
fn unknown_node(tag: &str, start: &BytesStart<'_>) -> Result<Option<SkippedNode>, XmlError> {
    let Some(name) = attribute_value(start, "Name")? else {
        return Ok(None);
    };
    let tag = tag.to_owned();
    tracing::warn!(
        tag = %tag,
        node = %name,
        "skipping node of an unsupported type"
    );
    Ok(Some(SkippedNode {
        error: format!("unsupported node type <{tag}>"),
        tag,
        name: Some(name),
    }))
}

/// Dispatch a node element to its parser. `start` must be the element's start
/// event and `reader` must be positioned just after it.
fn parse_node(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<Vec<NodeDecl>, XmlError> {
    let tag = start.name().as_ref().to_string();
    let node = match tag.as_str() {
        "Integer" | "IntReg" | "MaskedIntReg" => parse_integer(reader, start)?,
        "IntSwissKnife" | "SwissKnife" => parse_swissknife(reader, start)?,
        "Float" | "FloatReg" => parse_float(reader, start)?,
        "Enumeration" => parse_enum(reader, start)?,
        "Boolean" => parse_boolean(reader, start)?,
        "Command" => parse_command(reader, start)?,
        "Category" => parse_category(reader, start)?,
        "Converter" => parse_converter(reader, start)?,
        "IntConverter" => parse_int_converter(reader, start)?,
        "Register" => parse_register(reader, start)?,
        "StringReg" | "String" => parse_string(reader, start)?,
        "StructReg" => return parse_struct_reg(reader, start),
        other => {
            return Err(XmlError::Invalid(format!("not a node element: {other}")));
        }
    };
    Ok(vec![node])
}

/// Parse one node element in isolation, from a slice spanning the whole element.
///
/// A fresh reader over just this element means a parse failure cannot leave the
/// document-level reader at an unknown position — the caller has already
/// consumed exactly this element and is correctly positioned either way.
fn parse_isolated_node(element: &str) -> Result<Vec<NodeDecl>, XmlError> {
    let mut reader = Reader::from_str(element);
    reader.config_mut().trim_text(true);
    reader.config_mut().allow_dangling_amp = true;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let start = e.into_owned();
                return parse_node(&mut reader, start);
            }
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid("node element is empty".into()));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            // Leading whitespace or a comment before the start tag.
            _ => {}
        }
    }
}

/// Parse a GenICam XML document into an [`XmlModel`].
///
/// The parser only understands a practical subset of the schema. Unknown tags
/// are skipped which keeps the implementation forward compatible with richer
/// documents.
///
/// Each node element is parsed in isolation: one declaration we cannot make
/// sense of costs that single feature and is recorded in [`XmlModel::skipped`],
/// rather than failing the load and leaving the camera unopenable. Only errors
/// that make the document as a whole unreadable are returned.
pub fn parse(xml: &str) -> Result<XmlModel, XmlError> {
    let xml = strip_bom(xml);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Vendor XML is not ours to fix: a lone `&` in a tooltip must not stop us
    // from opening the camera.
    reader.config_mut().allow_dangling_amp = true;
    let mut buf = Vec::new();
    let mut version = String::from("0.0.0");
    let mut nodes = Vec::new();
    let mut skipped = Vec::new();

    loop {
        // Captured before the read so a node element's slice can be recovered
        // verbatim, start tag included.
        let element_start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "RegisterDescription" => {
                    version = schema_version_from(e)?;
                }
                "Group" => {
                    // Group is a transparent container wrapping feature nodes;
                    // let child events surface in the next loop iterations.
                }
                "Port" => {
                    // Port nodes are transport-level abstractions; skip them.
                    skip_element(&mut reader, e.name().as_ref())?;
                }
                tag if is_node_tag(tag) => {
                    let tag = tag.to_owned();
                    let name = attribute_value(e, "Name")?;
                    // Consume the element up front: whatever the node parser
                    // makes of it, the document reader stays in step.
                    reader
                        .read_to_end(QName(&tag))
                        .map_err(|err| XmlError::Xml(err.to_string()))?;
                    // `get`, not `[..]`: this slice is built from reader
                    // offsets rather than from `xml` itself, and a byte-order
                    // mark used to desynchronise the two (#122). Indexing
                    // panics on a non-character-boundary index; `get` turns the
                    // same disagreement into one skipped feature, which is the
                    // right price to pay in the middle of a camera connect.
                    let element = xml
                        .get(element_start..reader.buffer_position() as usize)
                        .ok_or_else(|| {
                            XmlError::Invalid(format!(
                                "reader offset {element_start}..{} is not a character \
                                 boundary in the document",
                                reader.buffer_position()
                            ))
                        });
                    match element.and_then(parse_isolated_node) {
                        Ok(parsed) => nodes.extend(parsed),
                        Err(err) => {
                            tracing::warn!(
                                tag = %tag,
                                node = name.as_deref().unwrap_or("<unnamed>"),
                                error = %err,
                                "skipping unparsable node"
                            );
                            skipped.push(SkippedNode {
                                tag,
                                name,
                                error: err.to_string(),
                            });
                        }
                    }
                }
                tag => {
                    let unknown = unknown_node(tag, e)?;
                    skip_element(&mut reader, e.name().as_ref())?;
                    if let Some(record) = unknown {
                        skipped.push(record);
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                "RegisterDescription" => {
                    version = schema_version_from(e)?;
                }
                "Command" => {
                    let node = parse_command_empty(e)?;
                    nodes.push(node);
                }
                "Category" => {
                    let node = parse_category_empty(e)?;
                    nodes.push(node);
                }
                // Same transport-level abstraction the Start arm skips; four
                // corpus documents declare it as `<Port Name="Device"/>`.
                "Port" => {}
                tag => {
                    if let Some(record) = unknown_node(tag, e)? {
                        skipped.push(record);
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if !skipped.is_empty() {
        tracing::warn!(
            count = skipped.len(),
            total = nodes.len() + skipped.len(),
            "some GenApi nodes could not be parsed and were skipped"
        );
    }

    Ok(XmlModel {
        version,
        nodes,
        skipped,
    })
}

fn schema_version_from(event: &BytesStart<'_>) -> Result<String, XmlError> {
    let major = attribute_value(event, "SchemaMajorVersion")?;
    let minor = attribute_value(event, "SchemaMinorVersion")?;
    let sub = attribute_value(event, "SchemaSubMinorVersion")?;
    let major = major.unwrap_or_else(|| "0".to_string());
    let minor = minor.unwrap_or_else(|| "0".to_string());
    let sub = sub.unwrap_or_else(|| "0".to_string());
    Ok(format!("{major}.{minor}.{sub}"))
}

fn handle_start(
    event: &BytesStart<'_>,
    depth: usize,
    schema_version: &mut Option<String>,
    top_level: &mut Vec<String>,
) -> Result<(), XmlError> {
    if depth == 1 && schema_version.is_none() {
        *schema_version = extract_schema_version(event);
    } else if depth == 2 {
        if let Some(name) = attribute_value(event, "Name")? {
            top_level.push(name);
        } else {
            top_level.push(event.name().as_ref().to_string());
        }
    }
    Ok(())
}

fn extract_schema_version(event: &BytesStart<'_>) -> Option<String> {
    let major = attribute_value(event, "SchemaMajorVersion").ok().flatten();
    let minor = attribute_value(event, "SchemaMinorVersion").ok().flatten();
    let sub = attribute_value(event, "SchemaSubMinorVersion")
        .ok()
        .flatten();
    if major.is_none() && minor.is_none() && sub.is_none() {
        None
    } else {
        let major = major.unwrap_or_else(|| "0".to_string());
        let minor = minor.unwrap_or_else(|| "0".to_string());
        let sub = sub.unwrap_or_else(|| "0".to_string());
        Some(format!("{major}.{minor}.{sub}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Category Name="Root">
                <pFeature>Gain</pFeature>
                <pFeature>GainSelector</pFeature>
            </Category>
            <Integer Name="Width">
                <Address>0x0000_0100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <Max>4096</Max>
                <Inc>2</Inc>
            </Integer>
            <Float Name="ExposureTime">
                <Address>0x0000_0200</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>10.0</Min>
                <Max>200000.0</Max>
                <Scale>1/1000</Scale>
                <Offset>0.0</Offset>
            </Float>
            <Enumeration Name="GainSelector">
                <Address>0x0000_0300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="AnalogAll" Value="0" />
                <EnumEntry Name="DigitalAll" Value="1" />
            </Enumeration>
            <Integer Name="Gain">
                <Address>0x0000_0304</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
                <pSelected>GainSelector</pSelected>
                <Selected>AnalogAll</Selected>
            </Integer>
            <Boolean Name="GammaEnable">
                <Address>0x0000_0400</Address>
                <Length>1</Length>
                <AccessMode>RW</AccessMode>
            </Boolean>
            <Command Name="AcquisitionStart">
                <Address>0x0000_0500</Address>
                <Length>4</Length>
            </Command>
        </RegisterDescription>
    "#;

    #[test]
    fn parse_minimal_xml() {
        let info = parse_into_minimal_nodes(FIXTURE).expect("parse xml");
        assert_eq!(info.schema_version.as_deref(), Some("1.2.3"));
        assert_eq!(info.top_level_features.len(), 7);
        assert_eq!(info.top_level_features[0], "Root");
    }

    #[test]
    fn parse_fixture_model() {
        let model = parse(FIXTURE).expect("parse fixture");
        assert_eq!(model.version, "1.2.3");
        assert_eq!(model.nodes.len(), 7);
        match &model.nodes[0] {
            NodeDecl::Category { name, children, .. } => {
                assert_eq!(name, "Root");
                assert_eq!(
                    children,
                    &vec!["Gain".to_string(), "GainSelector".to_string()]
                );
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[1] {
            NodeDecl::Integer {
                name,
                min,
                max,
                inc,
                ..
            } => {
                assert_eq!(name, "Width");
                assert_eq!(*min, 16);
                assert_eq!(*max, 4096);
                assert_eq!(*inc, Some(2));
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[2] {
            NodeDecl::Float {
                name,
                scale,
                offset,
                ..
            } => {
                assert_eq!(name, "ExposureTime");
                assert_eq!(*scale, Some((1, 1000)));
                assert_eq!(*offset, Some(0.0));
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[3] {
            NodeDecl::Enum { name, entries, .. } => {
                assert_eq!(name, "GainSelector");
                assert_eq!(entries.len(), 2);
                assert!(matches!(entries[0].value, EnumValueSrc::Literal(0)));
                assert!(matches!(entries[1].value, EnumValueSrc::Literal(1)));
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[4] {
            NodeDecl::Integer {
                name, selected_if, ..
            } => {
                assert_eq!(name, "Gain");
                assert_eq!(selected_if.len(), 1);
                assert_eq!(selected_if[0].0, "GainSelector");
                assert_eq!(selected_if[0].1, vec!["AnalogAll".to_string()]);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn parse_swissknife_node() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="GainRaw">
                    <Address>0x3000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>1000</Max>
                </Integer>
                <Float Name="Offset">
                    <Address>0x3008</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>-100.0</Min>
                    <Max>100.0</Max>
                </Float>
                <SwissKnife Name="ComputedGain">
                    <Expression>(GainRaw * 0.5) + Offset</Expression>
                    <pVariable Name="GainRaw">GainRaw</pVariable>
                    <pVariable Name="Offset">Offset</pVariable>
                    <Output>Float</Output>
                </SwissKnife>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse swissknife xml");
        assert_eq!(model.nodes.len(), 3);
        let swiss = model
            .nodes
            .iter()
            .find_map(|decl| match decl {
                NodeDecl::SwissKnife(node) => Some(node),
                _ => None,
            })
            .expect("swissknife present");
        assert_eq!(swiss.name, "ComputedGain");
        assert_eq!(swiss.expr, "(GainRaw * 0.5) + Offset");
        assert_eq!(swiss.output, SkOutput::Float);
        assert_eq!(swiss.variables.len(), 2);
        assert_eq!(
            swiss.variables[0],
            ("GainRaw".to_string(), "GainRaw".to_string())
        );
        assert_eq!(
            swiss.variables[1],
            ("Offset".to_string(), "Offset".to_string())
        );
    }

    #[test]
    fn parse_int_swissknife_with_hex_and_ampersand() {
        // Test that &amp; is decoded to & and hex literals are supported.
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <IntSwissKnife Name="PayloadSize">
                    <pVariable Name="W">Width</pVariable>
                    <pVariable Name="H">Height</pVariable>
                    <pVariable Name="PF">PixelFormat</pVariable>
                    <Formula>W * H * ((PF>>16)&amp;0xFF) / 8</Formula>
                </IntSwissKnife>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse intswissknife");
        assert_eq!(model.nodes.len(), 1);
        let swiss = model
            .nodes
            .iter()
            .find_map(|decl| match decl {
                NodeDecl::SwissKnife(node) => Some(node),
                _ => None,
            })
            .expect("swissknife present");
        assert_eq!(swiss.name, "PayloadSize");
        // &amp; should be decoded to &
        assert!(
            swiss.expr.contains('&'),
            "expression should contain decoded '&': {}",
            swiss.expr
        );
        assert!(
            swiss.expr.contains("0xFF"),
            "expression should contain hex literal: {}",
            swiss.expr
        );
    }

    #[test]
    fn parse_enum_entry_with_pvalue() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Enumeration Name="Mode">
                    <Address>0x0000_4000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <EnumEntry Name="Fixed10">
                        <Value>10</Value>
                    </EnumEntry>
                    <EnumEntry Name="DynFromReg">
                        <pValue>RegModeVal</pValue>
                    </EnumEntry>
                </Enumeration>
                <Integer Name="RegModeVal">
                    <Address>0x0000_4100</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse enum pvalue");
        assert_eq!(model.nodes.len(), 2);
        match &model.nodes[0] {
            NodeDecl::Enum { entries, .. } => {
                assert_eq!(entries.len(), 2);
                assert!(matches!(entries[0].value, EnumValueSrc::Literal(10)));
                match &entries[1].value {
                    EnumValueSrc::FromNode(node) => assert_eq!(node, "RegModeVal"),
                    other => panic!("unexpected entry value: {other:?}"),
                }
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// `<Address>` and `<pAddress>` on the same register are summed, in the
    /// order they were declared.
    ///
    /// FLIR and Point Grey write exactly this shape — `<pAddress>` for the
    /// block base, `<Address>` for the offset within it — and dropping either
    /// term reads the wrong register (issue #35).
    #[test]
    fn address_terms_are_summed() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegAddr">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                </Integer>
                <Integer Name="Gain">
                    <pAddress>RegAddr</pAddress>
                    <Address>0x00000008</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>255</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse indirect xml");
        assert_eq!(model.nodes.len(), 2);
        match &model.nodes[0] {
            NodeDecl::Integer {
                name, addressing, ..
            } => {
                assert_eq!(name, "RegAddr");
                assert_eq!(addressing.as_ref(), Some(&Addressing::fixed(0x2000, 4)));
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[1] {
            NodeDecl::Integer {
                name, addressing, ..
            } => {
                assert_eq!(name, "Gain");
                match addressing {
                    Some(Addressing::Sum { terms, len }) => {
                        assert_eq!(
                            terms,
                            &[AddressTerm::Node("RegAddr".into()), AddressTerm::Fixed(0x8),]
                        );
                        assert_eq!(*len, 4);
                    }
                    other => panic!("expected summed addressing, got {other:?}"),
                }
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// `<pIndex>` contributes an index scaled by its stride.
    #[test]
    fn parse_p_index_term() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <MaskedIntReg Name="RegTriggerInqDelay">
                    <Address>0x13400</Address>
                    <pIndex Offset="64">IntTriggerSelector</pIndex>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Bit>30</Bit>
                </MaskedIntReg>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse pIndex xml");
        match &model.nodes[0] {
            NodeDecl::Integer { addressing, .. } => match addressing {
                Some(Addressing::Sum { terms, .. }) => assert_eq!(
                    terms,
                    &[
                        AddressTerm::Fixed(0x13400),
                        AddressTerm::Index {
                            node: "IntTriggerSelector".into(),
                            offset: IndexOffset::Fixed(64),
                        },
                    ]
                ),
                other => panic!("expected summed addressing, got {other:?}"),
            },
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// A `<StructReg>` shares its address terms with every `<StructEntry>`.
    ///
    /// Point Grey declares `<Address>` next to `<pAddress>` here; before the
    /// additive model the base was dropped and all 38 inquiry bits in the
    /// document read register 0.
    #[test]
    fn struct_reg_inherits_all_address_terms() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <StructReg Comment="Gain Inquiry Register">
                    <Address>0x520</Address>
                    <pAddress>CamRegBaseAddress</pAddress>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Endianess>BigEndian</Endianess>
                    <StructEntry Name="GainPresInq_Bit"><Bit>0</Bit></StructEntry>
                    <StructEntry Name="GainAutoInq_Bit"><Bit>6</Bit></StructEntry>
                </StructReg>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse struct reg");
        assert_eq!(model.nodes.len(), 2);
        for node in &model.nodes {
            match node {
                NodeDecl::Integer { addressing, .. } => match addressing {
                    Some(Addressing::Sum { terms, len }) => {
                        assert_eq!(
                            terms,
                            &[
                                AddressTerm::Fixed(0x520),
                                AddressTerm::Node("CamRegBaseAddress".into()),
                            ]
                        );
                        assert_eq!(*len, 4);
                    }
                    other => panic!("expected summed addressing, got {other:?}"),
                },
                other => panic!("unexpected node: {other:?}"),
            }
        }
    }

    /// A `<StructReg>` with no address at all is dropped rather than silently
    /// pointed at register zero.
    #[test]
    fn struct_reg_without_address_is_skipped() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <StructReg>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <StructEntry Name="SomeInq_Bit"><Bit>0</Bit></StructEntry>
                </StructReg>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("document still parses");
        assert!(model.nodes.is_empty());
        assert_eq!(model.skipped.len(), 1);
        assert!(model.skipped[0].error.contains("Address"));
    }

    #[test]
    fn parse_indirect_float_is_scaled_integer() {
        // Regression test: indirect <Float> with Length=4 and no
        // <Scale>/<Offset> must NOT be reclassified as IEEE 754. Pointer-
        // backed float features on real cameras are almost always scaled
        // integer registers; silently decoding their bytes as IEEE 754
        // corrupts the value.
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegAddr">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                </Integer>
                <Float Name="Exposure">
                    <pAddress>RegAddr</pAddress>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                </Float>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse indirect float");
        let float = model
            .nodes
            .iter()
            .find(|n| matches!(n, NodeDecl::Float { name, .. } if name == "Exposure"))
            .expect("Exposure node");
        match float {
            NodeDecl::Float {
                encoding,
                addressing,
                ..
            } => {
                assert!(
                    matches!(
                        addressing,
                        Some(Addressing::Sum { terms, .. })
                            if terms.iter().any(|t| matches!(t, AddressTerm::Node(_)))
                    ),
                    "expected a pAddress term"
                );
                assert_eq!(*encoding, FloatEncoding::ScaledInteger);
            }
            _ => unreachable!(),
        }
    }

    /// Big-endian `<LSB>`/`<MSB>` are counted **from the MSB**, so a
    /// conformant document has `<LSB>` >= `<MSB>` and the pair's *minimum* is
    /// the offset of the field's most significant bit.
    ///
    /// The fixture is real: `AVT_Manta_G125B.xml` declares GigE Vision's
    /// bootstrap `GevSCPSPacketSize` at `0xD04` exactly this way, and the
    /// standard fixes that register's layout — the packet size is the *low*
    /// 16 bits. `viva_genapi::bitops` turns `bit_offset = 16` over 4 bytes into
    /// `shift = 32 - 16 - 16 = 0`, i.e. the low half. The end-to-end decode is
    /// asserted in `viva-genapi`, which owns the extraction.
    ///
    /// This test previously used `<Lsb>8</Lsb><Msb>15</Msb>` with `BigEndian` —
    /// a shape that appears **zero** times in the 38-document vendor corpus and
    /// is inverted against its own byte order. It encoded the issue-#120 defect
    /// rather than catching it.
    #[test]
    fn parse_integer_bitfield_big_endian() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegSCPSPacketSize">
                    <Address>0xD04</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <LSB>31</LSB>
                    <MSB>16</MSB>
                    <Endianess>BigEndian</Endianess>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse big-endian bitfield");
        assert_eq!(model.nodes.len(), 1);
        match &model.nodes[0] {
            NodeDecl::Integer { len, bitfield, .. } => {
                assert_eq!(*len, 4);
                let field = bitfield.as_ref().expect("bitfield present");
                assert_eq!(field.byte_order, ByteOrder::Big);
                assert_eq!(field.bit_length, 16);
                assert_eq!(field.bit_offset, 16);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// `<Bit>` is `<LSB>` and `<MSB>` at the same index, so it inherits the same
    /// orientation: on a big-endian register `<Bit>0</Bit>` is the *most*
    /// significant bit.
    ///
    /// This is the exact shape behind issue #120 — FLIR gates `ExposureTime` on
    /// three such registers sharing one 32-bit word at `0x000C1000`.
    #[test]
    fn parse_big_endian_single_bit_is_counted_from_the_msb() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="ExposureTime_Imp">
                    <Address>0x000C1000</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Bit>0</Bit>
                    <Endianess>BigEndian</Endianess>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse big-endian single bit");
        match &model.nodes[0] {
            NodeDecl::Integer { bitfield, .. } => {
                let field = bitfield.as_ref().expect("bitfield present");
                assert_eq!(field.bit_length, 1);
                // Offset from the MSB, so `bitops` shifts by 31 and reads the
                // top bit. Before #120 this was 31, which shifted by 0.
                assert_eq!(field.bit_offset, 0);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// The GenICam schema spells the bit-range elements `LSB` and `MSB`, and
    /// every one of the 1 419 declarations inside register nodes across the
    /// vendor corpus uses that spelling — none uses `Lsb`.
    ///
    /// `parsers::numeric` matched only the mixed-case form, so it dropped the
    /// bit range from every `<MaskedIntReg>` in every real document and read the
    /// whole register instead. `parsers::struct_reg` already accepted both,
    /// which is another way the two bitfield paths had drifted apart. Both
    /// spellings must parse, and to the same field.
    #[test]
    fn both_spellings_of_lsb_and_msb_are_accepted() {
        fn bitfield_of(lsb_tag: &str, msb_tag: &str) -> BitField {
            let xml = format!(
                r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">
                     <MaskedIntReg Name="Field">
                       <Address>0xD04</Address><Length>4</Length><AccessMode>RW</AccessMode>
                       <{lsb_tag}>31</{lsb_tag}><{msb_tag}>16</{msb_tag}>
                       <Endianess>BigEndian</Endianess>
                     </MaskedIntReg>
                   </RegisterDescription>"#
            );
            let model = parse(&xml).expect("parse");
            match &model.nodes[0] {
                NodeDecl::Integer { bitfield, .. } => *bitfield.as_ref().expect("bitfield present"),
                other => panic!("unexpected node: {other:?}"),
            }
        }

        let schema = bitfield_of("LSB", "MSB");
        let mixed = bitfield_of("Lsb", "Msb");
        assert_eq!(schema.bit_offset, 16);
        assert_eq!(schema.bit_length, 16);
        assert_eq!(schema, mixed);
    }

    /// Both spellings must reach the `<Boolean>` parser too, not only the
    /// numeric one.
    ///
    /// `<Boolean>` is parsed by `parsers::symbolic`, a different function from
    /// the `<Integer>`/`<MaskedIntReg>` path, and it matched the shared
    /// `TAG_LSB`/`TAG_MSB` constants. Renaming those to the schema spelling for
    /// GA-22 therefore *swapped* which spelling worked here instead of
    /// accepting both — and a register-backed Boolean wider than one byte whose
    /// bit range is dropped is not merely misread, it fails
    /// `Boolean node {name} requires explicit bitfield metadata` and is skipped
    /// outright. Caught in review on
    /// [#124](https://github.com/VitalyVorobyev/viva-genicam/pull/124).
    #[test]
    fn boolean_accepts_both_spellings_of_lsb_and_msb() {
        fn flag_bitfield(lsb_tag: &str, msb_tag: &str) -> BitField {
            let xml = format!(
                r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">
                     <Boolean Name="Flag">
                       <Address>0x2000</Address><Length>4</Length><AccessMode>RW</AccessMode>
                       <{lsb_tag}>31</{lsb_tag}><{msb_tag}>31</{msb_tag}>
                       <Endianess>BigEndian</Endianess>
                     </Boolean>
                   </RegisterDescription>"#
            );
            let model = parse(&xml).expect("parse");
            assert!(model.skipped.is_empty(), "skipped: {:?}", model.skipped);
            match &model.nodes[0] {
                NodeDecl::Boolean { bitfield, .. } => *bitfield.as_ref().expect("bitfield present"),
                other => panic!("unexpected node: {other:?}"),
            }
        }

        let schema = flag_bitfield("LSB", "MSB");
        let mixed = flag_bitfield("Lsb", "Msb");
        // Index 31 from the MSB of a 4-byte register is the least significant
        // bit, so `bitops` shifts by zero.
        assert_eq!(schema.bit_offset, 31);
        assert_eq!(schema.bit_length, 1);
        assert_eq!(schema, mixed);
    }

    /// `<Mask>` is the one bitfield source that stays LSB-relative under `Big`,
    /// because it is a literal register value rather than a GenICam bit index.
    ///
    /// It has zero corpus occurrences, so only this test keeps the distinction
    /// honest: the #120 fix removes the endianness conversion for `<LSB>`/
    /// `<MSB>`/`<Bit>` and must **keep** it here.
    #[test]
    fn big_endian_mask_stays_lsb_relative() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="Masked">
                    <Address>0x3000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Mask>0x0000FF00</Mask>
                    <Endianess>BigEndian</Endianess>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse big-endian mask");
        match &model.nodes[0] {
            NodeDecl::Integer { bitfield, .. } => {
                let field = bitfield.as_ref().expect("bitfield present");
                assert_eq!(field.byte_order, ByteOrder::Big);
                assert_eq!(field.bit_length, 8);
                // Bits 8..15 counted from the LSB are bits 16..23 from the MSB.
                assert_eq!(field.bit_offset, 16);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }
    #[test]
    fn parse_boolean_bitfield_default_length() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Boolean Name="Flag">
                    <Address>0x2000</Address>
                    <Length>1</Length>
                    <AccessMode>RW</AccessMode>
                    <Bit>3</Bit>
                </Boolean>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse boolean bitfield");
        assert_eq!(model.nodes.len(), 1);
        match &model.nodes[0] {
            NodeDecl::Boolean { len, bitfield, .. } => {
                assert_eq!(*len, 1);
                let bf = bitfield.as_ref().expect("bitfield present");
                assert_eq!(bf.byte_order, ByteOrder::Little);
                assert_eq!(bf.bit_length, 1);
                assert_eq!(bf.bit_offset, 3);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn parse_integer_bitfield_mask() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="Masked">
                    <Address>0x3000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                    <Mask>0x0000FF00</Mask>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse mask bitfield");
        assert_eq!(model.nodes.len(), 1);
        match &model.nodes[0] {
            NodeDecl::Integer { bitfield, .. } => {
                let field = bitfield.as_ref().expect("bitfield present");
                assert_eq!(field.byte_order, ByteOrder::Little);
                assert_eq!(field.bit_length, 8);
                assert_eq!(field.bit_offset, 8);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn parse_node_metadata() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="Width">
                    <Address>0x100</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>16</Min>
                    <Max>4096</Max>
                    <Visibility>Expert</Visibility>
                    <Description>Image width in pixels.</Description>
                    <ToolTip>Width of the acquired image</ToolTip>
                    <DisplayName>Image Width</DisplayName>
                    <Representation>Linear</Representation>
                </Integer>
                <Float Name="Gain">
                    <Address>0x200</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0.0</Min>
                    <Max>48.0</Max>
                    <Unit>dB</Unit>
                    <Visibility>Beginner</Visibility>
                    <Representation>Logarithmic</Representation>
                </Float>
                <Category Name="Root">
                    <Visibility>Guru</Visibility>
                    <Description>Top-level category</Description>
                    <pFeature>Width</pFeature>
                    <pFeature>Gain</pFeature>
                </Category>
                <Enumeration Name="PixelFormat">
                    <Address>0x300</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Visibility>Beginner</Visibility>
                    <ToolTip>Pixel format selector</ToolTip>
                    <EnumEntry Name="Mono8" Value="0" />
                </Enumeration>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse metadata xml");
        assert_eq!(model.nodes.len(), 4);

        // Integer with full metadata
        match &model.nodes[0] {
            NodeDecl::Integer { name, meta, .. } => {
                assert_eq!(name, "Width");
                assert_eq!(meta.visibility, Visibility::Expert);
                assert_eq!(meta.description.as_deref(), Some("Image width in pixels."));
                assert_eq!(meta.tooltip.as_deref(), Some("Width of the acquired image"));
                assert_eq!(meta.display_name.as_deref(), Some("Image Width"));
                assert_eq!(meta.representation, Some(Representation::Linear));
            }
            other => panic!("unexpected node: {other:?}"),
        }

        // Float with visibility + representation
        match &model.nodes[1] {
            NodeDecl::Float { name, meta, .. } => {
                assert_eq!(name, "Gain");
                assert_eq!(meta.visibility, Visibility::Beginner);
                assert_eq!(meta.representation, Some(Representation::Logarithmic));
                assert!(meta.description.is_none());
            }
            other => panic!("unexpected node: {other:?}"),
        }

        // Category with visibility + description
        match &model.nodes[2] {
            NodeDecl::Category { name, meta, .. } => {
                assert_eq!(name, "Root");
                assert_eq!(meta.visibility, Visibility::Guru);
                assert_eq!(meta.description.as_deref(), Some("Top-level category"));
            }
            other => panic!("unexpected node: {other:?}"),
        }

        // Enum with visibility + tooltip
        match &model.nodes[3] {
            NodeDecl::Enum { name, meta, .. } => {
                assert_eq!(name, "PixelFormat");
                assert_eq!(meta.visibility, Visibility::Beginner);
                assert_eq!(meta.tooltip.as_deref(), Some("Pixel format selector"));
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// Wrap a node fragment in a minimal document and parse it.
    fn parse_fragment(node: &str) -> XmlModel {
        let xml = format!(
            r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">{node}</RegisterDescription>"#
        );
        parse(&xml).expect("parse fragment")
    }

    /// Extract the metadata of the single node in a parsed fragment.
    fn only_meta(model: &XmlModel) -> &NodeMeta {
        match model.nodes.first().expect("one node") {
            NodeDecl::Integer { meta, .. } => meta,
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// Regression for issue #122: The Imaging Source's DMK 33GP2000e ships its
    /// GenApi XML with a UTF-8 byte-order mark.
    ///
    /// The BOM is valid UTF-8, so nothing upstream rejected it and `parse`
    /// returned `Ok` — with every node in the document skipped, because
    /// quick-xml strips the BOM from its own view without advancing
    /// `buffer_position`, leaving each sliced element three bytes short of its
    /// closing `>`. Assert the node count *and* an empty skip list: asserting
    /// `parse(..).is_ok()` alone passed throughout the bug.
    #[test]
    fn byte_order_mark_does_not_shift_node_slices() {
        let body = r#"<Integer Name="Width"><Address>0x100</Address><Length>4</Length></Integer><Integer Name="Height"><Address>0x104</Address><Length>4</Length></Integer>"#;
        let doc = format!(
            r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">{body}</RegisterDescription>"#
        );
        let with_bom = format!("\u{feff}{doc}");

        let plain = parse(&doc).expect("parse without BOM");
        let bom = parse(&with_bom).expect("parse with BOM");

        assert!(
            bom.skipped.is_empty(),
            "BOM document skipped nodes: {:?}",
            bom.skipped
        );
        assert_eq!(bom.nodes.len(), plain.nodes.len());
        assert_eq!(bom.version, plain.version);
    }

    /// The same document, through the offset-free scan `viva-camctl` and the
    /// `fetch_xml` example use first. This half always worked — which is why
    /// issue #122 reported 291 top-level features listed correctly and then
    /// every node failing. Pin it so the two entry points cannot diverge again.
    #[test]
    fn byte_order_mark_does_not_disturb_the_minimal_scan() {
        let doc = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1"><Category Name="Root"><pFeature>Width</pFeature></Category></RegisterDescription>"#;
        let with_bom = format!("\u{feff}{doc}");

        let info = parse_into_minimal_nodes(&with_bom).expect("minimal scan with BOM");
        assert_eq!(info.schema_version.as_deref(), Some("1.1.1"));
        assert_eq!(info.top_level_features, vec!["Root".to_string()]);
    }

    /// A BOM in a document that also carries multi-byte text.
    ///
    /// The three-byte shift lands on ASCII markup for any XML we have seen, so
    /// this is not a second failure mode — it is the same one, checked on
    /// content where a wrong slice would corrupt text rather than only truncate
    /// a tag. The `xml.get(..)` guard in [`parse`] is defensive on top of that:
    /// no document in the corpus reaches a non-character-boundary index, and it
    /// exists so that if one ever does the cost is one skipped feature instead
    /// of a panic in the middle of a camera connect.
    #[test]
    fn byte_order_mark_before_multibyte_text_keeps_the_text_intact() {
        let doc = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1"><Integer Name="Gain"><Address>0x100</Address><Length>4</Length><ToolTip>Verstärkung in dB — Meßwert</ToolTip></Integer></RegisterDescription>"#;
        let with_bom = format!("\u{feff}{doc}");

        let model = parse(&with_bom).expect("parse with BOM and multi-byte text");
        assert!(model.skipped.is_empty(), "skipped: {:?}", model.skipped);
        assert_eq!(
            only_meta(&model).tooltip.as_deref(),
            Some("Verstärkung in dB — Meßwert")
        );
    }

    /// Regression for issue #45: a FLIR BFS-PGE camera failed to open because a
    /// CDATA section in a text element was run through XML unescaping, where the
    /// literal `&` it legally contains has no `;` to terminate it.
    #[test]
    fn cdata_text_is_taken_literally() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <ToolTip><![CDATA[Gain in dB & raw units, 0 < x < 10]]></ToolTip>
               </Integer>"#,
        );
        assert_eq!(
            only_meta(&model).tooltip.as_deref(),
            Some("Gain in dB & raw units, 0 < x < 10")
        );
    }

    /// Non-conformant vendor XML with a lone `&` must not stop the document from
    /// loading — a cosmetic tooltip is never worth failing a camera connect over.
    #[test]
    fn dangling_ampersand_is_kept_verbatim() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <ToolTip>Exposure & gain</ToolTip>
               </Integer>"#,
        );
        assert_eq!(
            only_meta(&model).tooltip.as_deref(),
            Some("Exposure & gain")
        );
    }

    /// Comments are markup, not text: they are dropped, and a `&` inside one is
    /// not an entity reference.
    #[test]
    fn comments_inside_text_elements_are_dropped() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <Description>Analog <!-- R&D note --> gain</Description>
               </Integer>"#,
        );
        assert_eq!(
            only_meta(&model).description.as_deref(),
            Some("Analog  gain")
        );
    }

    /// Entity references still resolve, and the whitespace around them survives
    /// even though the reader splits character data at each reference.
    #[test]
    fn entity_references_resolve_and_preserve_spacing() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <ToolTip>A &amp; B &lt; C &gt; D &quot;E&quot; &apos;F&apos;</ToolTip>
               </Integer>"#,
        );
        assert_eq!(
            only_meta(&model).tooltip.as_deref(),
            Some(r#"A & B < C > D "E" 'F'"#)
        );
    }

    /// Numeric character references, both hexadecimal and decimal.
    #[test]
    fn character_references_resolve() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <ToolTip>&#x2014; dash &#8212;</ToolTip>
               </Integer>"#,
        );
        assert_eq!(only_meta(&model).tooltip.as_deref(), Some("— dash —"));
    }

    /// GenICam declares no DTD, so an entity we cannot resolve is kept as written
    /// rather than failing the document.
    #[test]
    fn unknown_entity_is_kept_as_written() {
        let model = parse_fragment(
            r#"<Integer Name="Gain">
                 <Address>0x100</Address>
                 <ToolTip>copyright &copy; vendor</ToolTip>
               </Integer>"#,
        );
        assert_eq!(
            only_meta(&model).tooltip.as_deref(),
            Some("copyright &copy; vendor")
        );
    }

    /// SwissKnife formulas escape their bitwise/comparison operators; the parsed
    /// expression must contain the operators, not the entities.
    #[test]
    fn swissknife_formula_entities_resolve_to_operators() {
        let model = parse_fragment(
            r#"<IntSwissKnife Name="GainMask">
                 <pVariable Name="RAW">Gain</pVariable>
                 <Formula>(RAW &amp; 0xFF) &lt; 16</Formula>
               </IntSwissKnife>"#,
        );
        match model.nodes.first().expect("one node") {
            NodeDecl::SwissKnife(node) => assert_eq!(node.expr, "(RAW & 0xFF) < 16"),
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// A SwissKnife whose formula is constant needs no `<pVariable>`. Rejecting
    /// these blocked a Hikrobot MV-CS050-10GC on `PixelDynamicRangeMin_Value`
    /// (reported in #35) and appears in the standard's own conformance document.
    #[test]
    fn swissknife_without_variables_is_accepted() {
        let model = parse_fragment(
            r#"<IntSwissKnife Name="PixelDynamicRangeMin_Value">
                 <Formula>0x1234</Formula>
               </IntSwissKnife>"#,
        );
        match model.nodes.first().expect("one node") {
            NodeDecl::SwissKnife(node) => {
                assert_eq!(node.expr, "0x1234");
                assert!(node.variables.is_empty());
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    /// `<AccessMode>` values beyond the standard's three spellings appear in
    /// third-party documents; an odd one must not cost the whole camera.
    #[test]
    fn access_mode_accepts_aliases_and_defaults_unknown_to_rw() {
        assert_eq!(AccessMode::parse("R").unwrap(), AccessMode::RO);
        assert_eq!(AccessMode::parse("W").unwrap(), AccessMode::WO);
        assert_eq!(AccessMode::parse("WR").unwrap(), AccessMode::RW);
        assert_eq!(AccessMode::parse(" ro ").unwrap(), AccessMode::RO);
        assert_eq!(AccessMode::parse("Bogus").unwrap(), AccessMode::RW);
    }

    /// A node we cannot parse costs that one feature, not the whole document —
    /// and the reader stays in step, so nodes after it still load.
    #[test]
    fn unparsable_node_is_skipped_not_fatal() {
        let model = parse_fragment(
            r#"<Integer Name="Before">
                 <Address>0x100</Address>
                 <Length>4</Length>
               </Integer>
               <Integer Name="Broken">
                 <Address>0xZZZZ</Address>
                 <Length>4</Length>
               </Integer>
               <Integer Name="After">
                 <Address>0x200</Address>
                 <Length>4</Length>
               </Integer>"#,
        );

        let names: Vec<&str> = model
            .nodes
            .iter()
            .map(|node| match node {
                NodeDecl::Integer { name, .. } => name.as_str(),
                other => panic!("unexpected node: {other:?}"),
            })
            .collect();
        assert_eq!(names, ["Before", "After"]);

        assert_eq!(model.skipped.len(), 1);
        let skipped = &model.skipped[0];
        assert_eq!(skipped.tag, "Integer");
        assert_eq!(skipped.name.as_deref(), Some("Broken"));
        assert!(
            skipped.error.contains("invalid hex"),
            "unexpected error: {}",
            skipped.error
        );
    }

    /// Isolation must survive a node whose failure happens before the parser has
    /// consumed any children — the outer reader is positioned by the caller, not
    /// by how far the node parser got.
    #[test]
    fn node_missing_required_name_is_skipped() {
        let model = parse_fragment(
            r#"<Integer>
                 <Address>0x100</Address>
               </Integer>
               <Integer Name="Good">
                 <Address>0x200</Address>
                 <Length>4</Length>
               </Integer>"#,
        );
        assert_eq!(model.nodes.len(), 1);
        assert_eq!(model.skipped.len(), 1);
        assert_eq!(model.skipped[0].name, None);
    }

    /// A clean document reports nothing skipped.
    #[test]
    fn clean_document_skips_nothing() {
        let model = parse(FIXTURE).expect("parse fixture");
        assert!(model.skipped.is_empty());
    }
}
