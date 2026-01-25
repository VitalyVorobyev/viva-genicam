#![cfg_attr(docsrs, feature(doc_cfg))]
//! Load and pre-parse GenICam XML using quick-xml.

use std::future::Future;

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader;
use thiserror::Error;
use tracing::warn;

const FIRST_URL_ADDRESS: u64 = 0x0000;
const FIRST_URL_MAX_LEN: usize = 512;

/// XML element name referencing another node that provides an address.
const TAG_P_ADDRESS: &[u8] = b"pAddress";
/// XML element holding an inline literal value.
const TAG_VALUE: &[u8] = b"Value";
/// XML element referencing another node supplying the value at runtime.
const TAG_P_VALUE: &[u8] = b"pValue";
/// XML element specifying a user friendly label for an enum entry.
const TAG_DISPLAY_NAME: &[u8] = b"DisplayName";
/// XML element describing the least significant bit of a bitfield.
const TAG_LSB: &[u8] = b"Lsb";
/// XML element describing the most significant bit of a bitfield.
const TAG_MSB: &[u8] = b"Msb";
/// XML element describing the starting bit index of a bitfield.
const TAG_BIT: &[u8] = b"Bit";
/// XML element describing a bitmask for a bitfield.
const TAG_MASK: &[u8] = b"Mask";
/// XML element providing the register byte order (common spelling).
const TAG_ENDIANNESS: &[u8] = b"Endianness";
/// XML element providing the register byte order (alternate spelling).
const TAG_ENDIANESS: &[u8] = b"Endianess";
/// XML element providing the register byte order (PFNC style).
const TAG_BYTE_ORDER: &[u8] = b"ByteOrder";

/// Source of the numeric value backing an enumeration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumValueSrc {
    /// Numeric literal declared directly in the XML.
    Literal(i64),
    /// Value obtained from another node referenced via `<pValue>`.
    FromNode(String),
}

/// Declaration for a single enumeration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumEntryDecl {
    /// Symbolic entry name exposed to clients.
    pub name: String,
    /// Source describing how to resolve the numeric value for this entry.
    pub value: EnumValueSrc,
    /// Optional user facing label.
    pub display_name: Option<String>,
}

#[derive(Debug, Error)]
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

/// Access privileges for a GenICam node as described in the XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Read-only node. The underlying register must not be modified by the client.
    RO,
    /// Write-only node. Reading the register is not permitted.
    WO,
    /// Read-write node. The register may be read and written by the client.
    RW,
}

impl AccessMode {
    fn parse(value: &str) -> Result<Self, XmlError> {
        match value.trim().to_ascii_uppercase().as_str() {
            "RO" => Ok(AccessMode::RO),
            "WO" => Ok(AccessMode::WO),
            "RW" => Ok(AccessMode::RW),
            other => Err(XmlError::Invalid(format!("unknown access mode: {other}"))),
        }
    }
}

/// Register addressing metadata for a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressing {
    /// Node uses a fixed register block regardless of selector state.
    Fixed { address: u64, len: u32 },
    /// Node switches between register blocks based on a selector value.
    BySelector {
        /// Name of the selector node controlling the address.
        selector: String,
        /// Mapping of selector value to `(address, length)` pair.
        map: Vec<(String, (u64, u32))>,
    },
    /// Node resolves its register block through another node providing the address.
    Indirect {
        /// Node providing the register address at runtime.
        p_address_node: String,
        /// Length of the target register block in bytes.
        len: u32,
    },
}

/// Byte order used to interpret a multi-byte register payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// The first byte contains the least significant bits.
    Little,
    /// The first byte contains the most significant bits.
    Big,
}

impl ByteOrder {
    fn parse(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "littleendian" => Some(ByteOrder::Little),
            "bigendian" => Some(ByteOrder::Big),
            _ => None,
        }
    }
}

/// Bitfield metadata describing a sub-range of a register payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitField {
    /// Starting bit offset within the interpreted register value.
    pub bit_offset: u16,
    /// Number of bits covered by the field.
    pub bit_length: u16,
    /// Byte order used when interpreting the enclosing register.
    pub byte_order: ByteOrder,
}

/// Output type of a SwissKnife expression node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    fn parse(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "integer" => Some(SkOutput::Integer),
            "float" => Some(SkOutput::Float),
            _ => None,
        }
    }
}

/// Declaration of a SwissKnife node consisting of an arithmetic expression.
#[derive(Debug, Clone)]
pub struct SwissKnifeDecl {
    /// Feature name exposed to clients.
    pub name: String,
    /// Raw expression string to be parsed by the runtime.
    pub expr: String,
    /// Mapping of variables used in the expression to provider node names.
    pub variables: Vec<(String, String)>,
    /// Desired output type (integer or float).
    pub output: SkOutput,
}

/// Declaration of a node extracted from the GenICam XML description.
#[derive(Debug, Clone)]
pub enum NodeDecl {
    /// Integer feature backed by a fixed register block.
    Integer {
        /// Feature name.
        name: String,
        /// Addressing metadata.
        addressing: Addressing,
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
        /// Selector nodes referencing this feature.
        selectors: Vec<String>,
        /// Selector gating rules in the form (selector name, allowed values).
        selected_if: Vec<(String, Vec<String>)>,
    },
    /// Floating point feature backed by an integer register with scaling.
    Float {
        name: String,
        addressing: Addressing,
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
    },
    /// Enumeration feature exposing a list of named integer values.
    Enum {
        name: String,
        addressing: Addressing,
        access: AccessMode,
        entries: Vec<EnumEntryDecl>,
        default: Option<String>,
        selectors: Vec<String>,
        selected_if: Vec<(String, Vec<String>)>,
    },
    /// Boolean feature backed by a single bit/byte register.
    Boolean {
        name: String,
        addressing: Addressing,
        len: u32,
        access: AccessMode,
        bitfield: BitField,
        selectors: Vec<String>,
        selected_if: Vec<(String, Vec<String>)>,
    },
    /// Command feature that triggers an action when written.
    Command {
        name: String,
        address: u64,
        len: u32,
    },
    /// Category used to organise features.
    Category { name: String, children: Vec<String> },
    /// Computed value backed by an arithmetic expression referencing other nodes.
    SwissKnife(SwissKnifeDecl),
}

/// Full XML model describing the GenICam schema version and all declared nodes.
#[derive(Debug, Clone)]
pub struct XmlModel {
    /// Combined schema version extracted from the RegisterDescription attributes.
    pub version: String,
    /// Flat list of node declarations present in the document.
    pub nodes: Vec<NodeDecl>,
}

/// Fetch the GenICam XML document using the provided memory reader closure.
///
/// The closure must return the requested number of bytes starting at the
/// provided address. It can internally perform chunked transfers.
pub async fn fetch_and_load_xml<F, Fut>(mut read_mem: F) -> Result<String, XmlError>
where
    F: FnMut(u64, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, XmlError>>,
{
    let url_bytes = read_mem(FIRST_URL_ADDRESS, FIRST_URL_MAX_LEN).await?;
    let url = first_cstring(&url_bytes)
        .ok_or_else(|| XmlError::Invalid("FirstURL register is empty".into()))?;
    let location = UrlLocation::parse(&url)?;
    match location {
        UrlLocation::Local { address, length } => {
            let xml_bytes = read_mem(address, length).await?;
            String::from_utf8(xml_bytes)
                .map_err(|err| XmlError::Xml(format!("invalid UTF-8: {err}")))
        }
        UrlLocation::LocalNamed(name) => Err(XmlError::Unsupported(format!(
            "named local URL '{name}' is not supported"
        ))),
        UrlLocation::Http(url) => Err(XmlError::Unsupported(format!(
            "HTTP retrieval is not implemented ({url})"
        ))),
        UrlLocation::File(path) => Err(XmlError::Unsupported(format!(
            "file URL '{path}' is not supported"
        ))),
    }
}

/// Parse a GenICam XML snippet and collect minimal metadata.
pub fn parse_into_minimal_nodes(xml: &str) -> Result<MinimalXmlInfo, XmlError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
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
                if depth > 0 {
                    depth = depth.saturating_sub(1);
                }
            }
            Ok(Event::End(_)) => {
                if depth > 0 {
                    depth = depth.saturating_sub(1);
                }
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

/// Parse a GenICam XML document into an [`XmlModel`].
///
/// The parser only understands a practical subset of the schema. Unknown tags
/// are skipped which keeps the implementation forward compatible with richer
/// documents.
pub fn parse(xml: &str) -> Result<XmlModel, XmlError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut version = String::from("0.0.0");
    let mut nodes = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"RegisterDescription" => {
                    version = schema_version_from(e)?;
                }
                b"Integer" => {
                    let node = parse_integer(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"Float" => {
                    let node = parse_float(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"Enumeration" => {
                    let node = parse_enum(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"Boolean" => {
                    let node = parse_boolean(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"Command" => {
                    let node = parse_command(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"Category" => {
                    let node = parse_category(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                b"SwissKnife" => {
                    let node = parse_swissknife(&mut reader, e.clone())?;
                    nodes.push(node);
                }
                _ => {
                    skip_element(&mut reader, e.name().as_ref())?;
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"RegisterDescription" => {
                    version = schema_version_from(e)?;
                }
                b"Command" => {
                    let node = parse_command_empty(e)?;
                    nodes.push(node);
                }
                b"Category" => {
                    let node = parse_category_empty(e)?;
                    nodes.push(node);
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(XmlModel { version, nodes })
}

fn schema_version_from(event: &BytesStart<'_>) -> Result<String, XmlError> {
    let major = attribute_value(event, b"SchemaMajorVersion")?;
    let minor = attribute_value(event, b"SchemaMinorVersion")?;
    let sub = attribute_value(event, b"SchemaSubMinorVersion")?;
    let major = major.unwrap_or_else(|| "0".to_string());
    let minor = minor.unwrap_or_else(|| "0".to_string());
    let sub = sub.unwrap_or_else(|| "0".to_string());
    Ok(format!("{major}.{minor}.{sub}"))
}

#[derive(Debug, Default)]
struct AddressingBuilder {
    fixed_address: Option<u64>,
    length: Option<u32>,
    selector: Option<String>,
    entries: Vec<AddressEntry>,
    pending_value: Option<String>,
    pending_len: Option<u32>,
    p_address_node: Option<String>,
}

#[derive(Debug, Clone)]
struct AddressEntry {
    value: String,
    address: u64,
    len: Option<u32>,
}

impl AddressingBuilder {
    fn set_fixed_address(&mut self, address: u64) {
        self.fixed_address = Some(address);
    }

    fn set_length(&mut self, len: u32) {
        self.length = Some(len);
    }

    fn set_p_address_node(&mut self, node: &str) {
        self.p_address_node = Some(node.to_string());
    }

    fn register_selector(&mut self, selector: &str) {
        if self.selector.is_none() {
            self.selector = Some(selector.to_string());
        }
    }

    fn push_selected_value(&mut self, value: String) {
        self.pending_value = Some(value);
        self.pending_len = None;
    }

    fn apply_length(&mut self, len: u32) {
        if self.pending_value.is_some() {
            self.pending_len = Some(len);
        } else {
            self.length = Some(len);
        }
    }

    fn attach_selected_address(&mut self, address: u64, len_override: Option<u32>) {
        if let Some(value) = self.pending_value.take() {
            let len = len_override.or(self.pending_len.take());
            self.entries.push(AddressEntry {
                value,
                address,
                len,
            });
        } else {
            self.fixed_address = Some(address);
            if let Some(len) = len_override {
                self.length = Some(len);
            }
        }
    }

    fn finalize(self, node: &str, default_len: Option<u32>) -> Result<Addressing, XmlError> {
        if !self.entries.is_empty() {
            let selector = self.selector.ok_or_else(|| {
                XmlError::Invalid(format!(
                    "node {node} provides <Selected> addresses without <pSelected>"
                ))
            })?;
            let mut map = Vec::new();
            for entry in self.entries {
                let len = entry.len.or(self.length).or(default_len).ok_or_else(|| {
                    XmlError::Invalid(format!(
                        "node {node} is missing <Length> for selector value {}",
                        entry.value
                    ))
                })?;
                if let Some(existing) = map.iter_mut().find(|(value, _)| *value == entry.value) {
                    *existing = (entry.value.clone(), (entry.address, len));
                } else {
                    map.push((entry.value.clone(), (entry.address, len)));
                }
            }
            if self.p_address_node.is_some() {
                warn!(
                    node = %node,
                    "ignoring <pAddress> in favour of selector table"
                );
            }
            if self.fixed_address.is_some() {
                warn!(
                    node = %node,
                    selector = %selector,
                    "ignoring fixed <Address> in favour of selector table"
                );
            }
            Ok(Addressing::BySelector { selector, map })
        } else {
            let len = self
                .length
                .or(default_len)
                .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Length>")))?;
            if let Some(p_address_node) = self.p_address_node {
                if self.fixed_address.is_some() {
                    warn!(
                        node = %node,
                        address_node = %p_address_node,
                        "ignoring fixed <Address> in favour of <pAddress>"
                    );
                }
                Ok(Addressing::Indirect {
                    p_address_node,
                    len,
                })
            } else {
                let address = self.fixed_address.ok_or_else(|| {
                    XmlError::Invalid(format!("node {node} is missing <Address>"))
                })?;
                Ok(Addressing::Fixed { address, len })
            }
        }
    }
}

/// Tracks selector state during node parsing.
///
/// This struct consolidates the `selectors`, `selected_if`, and `last_selector`
/// variables that were previously duplicated across all node parsers.
#[derive(Debug, Default)]
struct SelectorState {
    /// List of selector node names referencing this feature.
    selectors: Vec<String>,
    /// Selector gating rules in the form (selector name, allowed values).
    selected_if: Vec<(String, Vec<String>)>,
    /// Index into `selected_if` for the most recent pSelected element.
    last_selector: Option<usize>,
}

impl SelectorState {
    /// Finalize into the component parts for NodeDecl construction.
    fn into_parts(self) -> (Vec<String>, Vec<(String, Vec<String>)>) {
        (self.selectors, self.selected_if)
    }
}

/// Handle a `<pSelected>` start element.
///
/// Reads the text content, registers the selector with addressing, and updates selector state.
fn handle_p_selected_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    addressing: &mut AddressingBuilder,
    state: &mut SelectorState,
) -> Result<(), XmlError> {
    let text = read_text_start(reader, event)?;
    let selector = text.trim().to_string();
    if !selector.is_empty() {
        state.selectors.push(selector.clone());
        state.selected_if.push((selector.clone(), Vec::new()));
        state.last_selector = Some(state.selected_if.len() - 1);
        addressing.register_selector(&selector);
    }
    Ok(())
}

/// Handle a `<Selected>` start element.
///
/// Parses value from attributes or text content, updates addressing and selector state.
fn handle_selected_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    name: &str,
    addressing: &mut AddressingBuilder,
    state: &mut SelectorState,
) -> Result<(), XmlError> {
    let mut value = attribute_value(event, b"Value")?;
    if value.is_none() {
        value = attribute_value(event, b"Name")?;
    }
    let text = read_text_start(reader, event)?;
    let trimmed = text.trim();
    if value.is_none() && !trimmed.is_empty() {
        value = Some(trimmed.to_string());
    }
    if let Some(val) = value.clone() {
        addressing.push_selected_value(val.clone());
        if let Some(address_attr) = attribute_value(event, b"Address")? {
            let len_override = attribute_value(event, b"Length")?
                .map(|len| -> Result<u32, XmlError> {
                    let parsed = parse_u64(&len)?;
                    u32::try_from(parsed).map_err(|_| {
                        XmlError::Invalid(format!("length out of range for node {name}"))
                    })
                })
                .transpose()?;
            addressing.attach_selected_address(parse_u64(&address_attr)?, len_override);
        } else if let Some(len_attr) = attribute_value(event, b"Length")? {
            let parsed = parse_u64(&len_attr)?;
            let len = u32::try_from(parsed)
                .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
            addressing.apply_length(len);
        }
    }
    if let Some(idx) = state.last_selector {
        if let Some(value) = value {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                state.selected_if[idx].1.push(trimmed.to_string());
            }
        } else if !trimmed.is_empty() {
            state.selected_if[idx].1.push(trimmed.to_string());
        }
    }
    Ok(())
}

/// Handle a `<pSelected>` empty element.
///
/// Reads selector name from attributes and updates state.
fn handle_p_selected_empty(
    event: &BytesStart<'_>,
    addressing: &mut AddressingBuilder,
    state: &mut SelectorState,
) -> Result<(), XmlError> {
    if let Some(value) = attribute_value(event, b"Name")? {
        addressing.register_selector(&value);
        state.selectors.push(value.clone());
        state.selected_if.push((value, Vec::new()));
        state.last_selector = Some(state.selected_if.len() - 1);
    }
    Ok(())
}

/// Handle a `<Selected>` empty element.
///
/// Parses value and addressing from attributes, updates state.
fn handle_selected_empty(
    event: &BytesStart<'_>,
    name: &str,
    addressing: &mut AddressingBuilder,
    state: &mut SelectorState,
) -> Result<(), XmlError> {
    if let Some(val) = attribute_value(event, b"Value")? {
        addressing.push_selected_value(val.clone());
        if let Some(address_attr) = attribute_value(event, b"Address")? {
            let len_override = attribute_value(event, b"Length")?
                .map(|len| -> Result<u32, XmlError> {
                    let parsed = parse_u64(&len)?;
                    u32::try_from(parsed).map_err(|_| {
                        XmlError::Invalid(format!("length out of range for node {name}"))
                    })
                })
                .transpose()?;
            addressing.attach_selected_address(parse_u64(&address_attr)?, len_override);
        } else if let Some(len_attr) = attribute_value(event, b"Length")? {
            let parsed = parse_u64(&len_attr)?;
            let len = u32::try_from(parsed)
                .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
            addressing.apply_length(len);
        }
        if let Some(idx) = state.last_selector {
            state.selected_if[idx].1.push(val);
        }
    }
    Ok(())
}

/// Handle common addressing elements for start events.
///
/// Returns `true` if the element was handled, `false` otherwise.
fn handle_addressing_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    name: &str,
    addressing: &mut AddressingBuilder,
) -> Result<bool, XmlError> {
    match event.name().as_ref() {
        b"Address" => {
            let text = read_text_start(reader, event)?;
            addressing.attach_selected_address(parse_u64(&text)?, None);
            Ok(true)
        }
        TAG_P_ADDRESS => {
            let text = read_text_start(reader, event)?;
            let target = text.trim();
            if !target.is_empty() {
                addressing.set_p_address_node(target);
            }
            Ok(true)
        }
        b"Length" => {
            let text = read_text_start(reader, event)?;
            let value = parse_u64(&text)?;
            let len = u32::try_from(value)
                .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
            addressing.apply_length(len);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Handle common addressing elements for empty events.
///
/// Returns `true` if the element was handled, `false` otherwise.
fn handle_addressing_empty(
    event: &BytesStart<'_>,
    addressing: &mut AddressingBuilder,
) -> Result<bool, XmlError> {
    match event.name().as_ref() {
        TAG_P_ADDRESS => {
            if let Some(value) = attribute_value(event, b"Name")? {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    addressing.set_p_address_node(trimmed);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitfieldSource {
    LsbMsb,
    BitLength,
    Mask,
}

#[derive(Debug, Default)]
struct BitfieldBuilder {
    lsb: Option<u32>,
    msb: Option<u32>,
    bit: Option<u32>,
    bit_length: Option<u32>,
    mask: Option<u64>,
    byte_order: Option<ByteOrder>,
    source: Option<BitfieldSource>,
}

impl BitfieldBuilder {
    fn note_lsb(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::LsbMsb)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::LsbMsb);
        self.lsb = Some(value);
    }

    fn note_msb(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::LsbMsb)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::LsbMsb);
        self.msb = Some(value);
    }

    fn note_bit(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::BitLength)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::BitLength);
        self.bit = Some(value);
    }

    fn note_bit_length(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::BitLength)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::BitLength);
        self.bit_length = Some(value);
    }

    fn note_mask(&mut self, mask: u64) {
        if self.source.is_some() {
            return;
        }
        self.source = Some(BitfieldSource::Mask);
        self.mask = Some(mask);
    }

    fn note_byte_order(&mut self, order: ByteOrder) {
        self.byte_order = Some(order);
    }

    fn finish(self, node: &str, lengths: &[u32]) -> Result<Option<BitField>, XmlError> {
        let source = match self.source {
            Some(source) => source,
            None => return Ok(None),
        };
        let byte_order = self.byte_order.unwrap_or(ByteOrder::Little);
        if lengths.is_empty() {
            return Err(XmlError::Invalid(format!(
                "node {node} is missing register length information"
            )));
        }
        let mut unique_len = None;
        for len in lengths {
            if *len == 0 {
                return Err(XmlError::Invalid(format!(
                    "node {node} declares zero-length register"
                )));
            }
            if let Some(existing) = unique_len {
                if existing != *len {
                    return Err(XmlError::Invalid(format!(
                        "node {node} uses inconsistent register lengths"
                    )));
                }
            } else {
                unique_len = Some(*len);
            }
        }
        let len_bytes = unique_len.unwrap_or(0);
        let total_bits = len_bytes
            .checked_mul(8)
            .ok_or_else(|| XmlError::Invalid(format!("node {node} register length overflow")))?;

        let (offset_lsb, bit_length) = match source {
            BitfieldSource::LsbMsb => {
                let lsb = self
                    .lsb
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Lsb>")))?;
                let msb = self
                    .msb
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Msb>")))?;
                let lower = lsb.min(msb);
                let upper = lsb.max(msb);
                let length = upper
                    .checked_sub(lower)
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| {
                        XmlError::Invalid(format!(
                            "node {node} has invalid bit range <Lsb>={lsb}, <Msb>={msb}"
                        ))
                    })?;
                (lower, length)
            }
            BitfieldSource::BitLength => {
                let bit = self
                    .bit
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Bit>")))?;
                let length = self.bit_length.unwrap_or(1);
                (bit, length)
            }
            BitfieldSource::Mask => {
                let mask = self.mask.ok_or_else(|| {
                    XmlError::Invalid(format!("node {node} is missing <Mask> value"))
                })?;
                if mask == 0 {
                    return Err(XmlError::Invalid(format!(
                        "node {node} mask must be non-zero"
                    )));
                }
                let offset = mask.trailing_zeros();
                let length = mask.count_ones();
                (offset, length)
            }
        };

        if bit_length == 0 {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield must have positive length"
            )));
        }
        if bit_length > 64 {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield length {bit_length} exceeds 64 bits"
            )));
        }

        if offset_lsb > u16::MAX as u32 {
            return Err(XmlError::Invalid(format!(
                "node {node} bit offset {offset_lsb} exceeds u16 range"
            )));
        }

        if bit_length > u16::MAX as u32 {
            return Err(XmlError::Invalid(format!(
                "node {node} bit length {bit_length} exceeds u16 range"
            )));
        }

        if offset_lsb + bit_length > total_bits {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield exceeds register width"
            )));
        }

        let offset = match byte_order {
            ByteOrder::Little => offset_lsb,
            ByteOrder::Big => total_bits - bit_length - offset_lsb,
        };

        Ok(Some(BitField {
            bit_offset: u16::try_from(offset).map_err(|_| {
                XmlError::Invalid(format!("node {node} bit offset {offset} exceeds u16 range"))
            })?,
            bit_length: u16::try_from(bit_length).map_err(|_| {
                XmlError::Invalid(format!(
                    "node {node} bit length {bit_length} exceeds u16 range"
                ))
            })?,
            byte_order,
        }))
    }
}

fn addressing_lengths(addressing: &Addressing) -> Vec<u32> {
    match addressing {
        Addressing::Fixed { len, .. } => vec![*len],
        Addressing::Indirect { len, .. } => vec![*len],
        Addressing::BySelector { map, .. } => map.iter().map(|(_, (_, len))| *len).collect(),
    }
}

fn parse_integer(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, b"Address")? {
        addressing.set_fixed_address(parse_u64(&addr)?);
    }
    if let Some(len) = attribute_value(&start, b"Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut min = None;
    let mut max = None;
    let mut inc = None;
    let mut unit = None;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut bitfield = BitfieldBuilder::default();
    let mut pending_bit_length = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" => {
                    let text = read_text_start(reader, e)?;
                    addressing.attach_selected_address(parse_u64(&text)?, None);
                }
                TAG_P_ADDRESS => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        addressing.set_p_address_node(target);
                    }
                }
                b"Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let mut handled = false;
                    if pending_bit_length {
                        if let Ok(bit_len) = u32::try_from(value) {
                            bitfield.note_bit_length(bit_len);
                            pending_bit_length = false;
                            handled = true;
                        } else {
                            return Err(XmlError::Invalid(format!(
                                "bitfield length out of range for node {name}"
                            )));
                        }
                    }
                    if !handled {
                        let len = u32::try_from(value).map_err(|_| {
                            XmlError::Invalid(format!("length out of range for node {name}"))
                        })?;
                        addressing.apply_length(len);
                    }
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                b"Min" => {
                    let text = read_text_start(reader, e)?;
                    min = Some(parse_i64(&text)?);
                }
                b"Max" => {
                    let text = read_text_start(reader, e)?;
                    max = Some(parse_i64(&text)?);
                }
                b"Inc" => {
                    let text = read_text_start(reader, e)?;
                    inc = Some(parse_i64(&text)?);
                }
                b"Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                TAG_LSB => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let lsb = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Lsb> out of range for node {name}"))
                    })?;
                    bitfield.note_lsb(lsb);
                }
                TAG_MSB => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let msb = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Msb> out of range for node {name}"))
                    })?;
                    bitfield.note_msb(msb);
                }
                TAG_BIT => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let bit = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Bit> out of range for node {name}"))
                    })?;
                    bitfield.note_bit(bit);
                    pending_bit_length = true;
                }
                TAG_MASK => {
                    let text = read_text_start(reader, e)?;
                    let mask = parse_u64(&text)?;
                    bitfield.note_mask(mask);
                    pending_bit_length = false;
                }
                TAG_ENDIANNESS | TAG_ENDIANESS | TAG_BYTE_ORDER => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        bitfield.note_byte_order(order);
                    }
                }
                b"pSelected" => {
                    handle_p_selected_start(reader, e, &mut addressing, &mut selector_state)?;
                }
                b"Selected" => {
                    handle_selected_start(reader, e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"pSelected" => {
                    handle_p_selected_empty(e, &mut addressing, &mut selector_state)?;
                }
                TAG_P_ADDRESS => {
                    handle_addressing_empty(e, &mut addressing)?;
                }
                TAG_LSB => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let lsb = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Lsb> out of range for node {name}"))
                        })?;
                        bitfield.note_lsb(lsb);
                    }
                }
                TAG_MSB => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let msb = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Msb> out of range for node {name}"))
                        })?;
                        bitfield.note_msb(msb);
                    }
                }
                TAG_BIT => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let bit = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Bit> out of range for node {name}"))
                        })?;
                        bitfield.note_bit(bit);
                        pending_bit_length = true;
                    }
                }
                TAG_MASK => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let mask = parse_u64(&value)?;
                        bitfield.note_mask(mask);
                        pending_bit_length = false;
                    }
                }
                TAG_ENDIANNESS | TAG_ENDIANESS | TAG_BYTE_ORDER => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        if let Some(order) = ByteOrder::parse(&value) {
                            bitfield.note_byte_order(order);
                        }
                    }
                }
                b"Selected" => {
                    handle_selected_empty(e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Integer node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let min =
        min.ok_or_else(|| XmlError::Invalid(format!("Integer node {name} is missing <Min>")))?;
    let max =
        max.ok_or_else(|| XmlError::Invalid(format!("Integer node {name} is missing <Max>")))?;

    let addressing = addressing.finalize(&name, Some(4))?;
    let lengths = addressing_lengths(&addressing);
    let len = lengths
        .first()
        .copied()
        .ok_or_else(|| XmlError::Invalid(format!("node {name} is missing <Length>")))?;
    let bitfield = bitfield.finish(&name, &lengths)?;
    let (selectors, selected_if) = selector_state.into_parts();

    Ok(NodeDecl::Integer {
        name,
        addressing,
        len,
        access,
        min,
        max,
        inc,
        unit,
        bitfield,
        selectors,
        selected_if,
    })
}

fn parse_float(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, b"Address")? {
        addressing.set_fixed_address(parse_u64(&addr)?);
    }
    if let Some(len) = attribute_value(&start, b"Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut min = None;
    let mut max = None;
    let mut unit = None;
    let mut scale_num: Option<i64> = None;
    let mut scale_den: Option<i64> = None;
    let mut offset = None;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" | TAG_P_ADDRESS | b"Length" => {
                    if !handle_addressing_start(reader, e, &name, &mut addressing)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                b"Min" => {
                    let text = read_text_start(reader, e)?;
                    min = Some(parse_f64(&text)?);
                }
                b"Max" => {
                    let text = read_text_start(reader, e)?;
                    max = Some(parse_f64(&text)?);
                }
                b"Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                b"Scale" => {
                    let text = read_text_start(reader, e)?;
                    let (num, den) = parse_scale(&text)?;
                    scale_num = Some(num);
                    scale_den = Some(den);
                }
                b"ScaleNumerator" => {
                    let text = read_text_start(reader, e)?;
                    scale_num = Some(parse_i64(&text)?);
                }
                b"ScaleDenominator" => {
                    let text = read_text_start(reader, e)?;
                    scale_den = Some(parse_i64(&text)?);
                }
                b"Offset" => {
                    let text = read_text_start(reader, e)?;
                    offset = Some(parse_f64(&text)?);
                }
                b"pSelected" => {
                    handle_p_selected_start(reader, e, &mut addressing, &mut selector_state)?;
                }
                b"Selected" => {
                    handle_selected_start(reader, e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"pSelected" => {
                    handle_p_selected_empty(e, &mut addressing, &mut selector_state)?;
                }
                TAG_P_ADDRESS => {
                    handle_addressing_empty(e, &mut addressing)?;
                }
                b"Selected" => {
                    handle_selected_empty(e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!("unterminated Float node {name}")))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let min =
        min.ok_or_else(|| XmlError::Invalid(format!("Float node {name} is missing <Min>")))?;
    let max =
        max.ok_or_else(|| XmlError::Invalid(format!("Float node {name} is missing <Max>")))?;
    let scale = match (scale_num, scale_den) {
        (Some(num), Some(den)) if den != 0 => Some((num, den)),
        (None, None) => None,
        (Some(num), None) => Some((num, 1)),
        _ => None,
    };

    let addressing = addressing.finalize(&name, Some(8))?;
    let (selectors, selected_if) = selector_state.into_parts();

    Ok(NodeDecl::Float {
        name,
        addressing,
        access,
        min,
        max,
        unit,
        scale,
        offset,
        selectors,
        selected_if,
    })
}

fn parse_enum(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, b"Address")? {
        addressing.set_fixed_address(parse_u64(&addr)?);
    }
    if let Some(len) = attribute_value(&start, b"Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut entries = Vec::new();
    let mut default = None;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" | TAG_P_ADDRESS | b"Length" => {
                    if !handle_addressing_start(reader, e, &name, &mut addressing)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                b"EnumEntry" => {
                    let entry = parse_enum_entry(reader, e.clone())?;
                    entries.push(entry);
                }
                b"pSelected" => {
                    handle_p_selected_start(reader, e, &mut addressing, &mut selector_state)?;
                }
                b"Selected" => {
                    handle_selected_start(reader, e, &name, &mut addressing, &mut selector_state)?;
                }
                b"pValueDefault" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        default = Some(trimmed.to_string());
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"EnumEntry" => {
                    let entry = parse_enum_entry_empty(e)?;
                    entries.push(entry);
                }
                b"pSelected" => {
                    handle_p_selected_empty(e, &mut addressing, &mut selector_state)?;
                }
                b"Selected" => {
                    handle_selected_empty(e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Enumeration node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if entries.is_empty() {
        return Err(XmlError::Invalid(format!(
            "Enumeration node {name} declares no <EnumEntry> elements"
        )));
    }

    let addressing = addressing.finalize(&name, Some(4))?;
    let (selectors, selected_if) = selector_state.into_parts();

    Ok(NodeDecl::Enum {
        name,
        addressing,
        access,
        entries,
        default,
        selectors,
        selected_if,
    })
}

fn parse_boolean(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, b"Address")? {
        addressing.set_fixed_address(parse_u64(&addr)?);
    }
    if let Some(len) = attribute_value(&start, b"Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::Invalid(format!("length out of range for node {name}")))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut bitfield = BitfieldBuilder::default();
    let mut pending_bit_length = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" => {
                    let text = read_text_start(reader, e)?;
                    addressing.attach_selected_address(parse_u64(&text)?, None);
                }
                TAG_P_ADDRESS => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        addressing.set_p_address_node(target);
                    }
                }
                b"Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let mut handled = false;
                    if pending_bit_length {
                        if let Ok(bit_len) = u32::try_from(value) {
                            bitfield.note_bit_length(bit_len);
                            pending_bit_length = false;
                            handled = true;
                        } else {
                            return Err(XmlError::Invalid(format!(
                                "bitfield length out of range for node {name}"
                            )));
                        }
                    }
                    if !handled {
                        let len = u32::try_from(value).map_err(|_| {
                            XmlError::Invalid(format!("length out of range for node {name}"))
                        })?;
                        addressing.apply_length(len);
                    }
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                TAG_LSB => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let lsb = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Lsb> out of range for node {name}"))
                    })?;
                    bitfield.note_lsb(lsb);
                }
                TAG_MSB => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let msb = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Msb> out of range for node {name}"))
                    })?;
                    bitfield.note_msb(msb);
                }
                TAG_BIT => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let bit = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("<Bit> out of range for node {name}"))
                    })?;
                    bitfield.note_bit(bit);
                    pending_bit_length = true;
                }
                TAG_MASK => {
                    let text = read_text_start(reader, e)?;
                    let mask = parse_u64(&text)?;
                    bitfield.note_mask(mask);
                    pending_bit_length = false;
                }
                TAG_ENDIANNESS | TAG_ENDIANESS | TAG_BYTE_ORDER => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        bitfield.note_byte_order(order);
                    }
                }
                b"pSelected" => {
                    handle_p_selected_start(reader, e, &mut addressing, &mut selector_state)?;
                }
                b"Selected" => {
                    handle_selected_start(reader, e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"pSelected" => {
                    handle_p_selected_empty(e, &mut addressing, &mut selector_state)?;
                }
                TAG_P_ADDRESS => {
                    handle_addressing_empty(e, &mut addressing)?;
                }
                TAG_LSB => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let lsb = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Lsb> out of range for node {name}"))
                        })?;
                        bitfield.note_lsb(lsb);
                    }
                }
                TAG_MSB => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let msb = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Msb> out of range for node {name}"))
                        })?;
                        bitfield.note_msb(msb);
                    }
                }
                TAG_BIT => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let bit = u32::try_from(parsed).map_err(|_| {
                            XmlError::Invalid(format!("<Bit> out of range for node {name}"))
                        })?;
                        bitfield.note_bit(bit);
                        pending_bit_length = true;
                    }
                }
                TAG_MASK => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let mask = parse_u64(&value)?;
                        bitfield.note_mask(mask);
                        pending_bit_length = false;
                    }
                }
                TAG_ENDIANNESS | TAG_ENDIANESS | TAG_BYTE_ORDER => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        if let Some(order) = ByteOrder::parse(&value) {
                            bitfield.note_byte_order(order);
                        }
                    }
                }
                b"Selected" => {
                    handle_selected_empty(e, &name, &mut addressing, &mut selector_state)?;
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Boolean node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let addressing = addressing.finalize(&name, Some(4))?;
    let lengths = addressing_lengths(&addressing);
    let len = lengths
        .first()
        .copied()
        .ok_or_else(|| XmlError::Invalid(format!("node {name} is missing <Length>")))?;
    let bitfield = match bitfield.finish(&name, &lengths)? {
        Some(field) => field,
        None if len == 1 => BitField {
            bit_offset: 0,
            bit_length: 1,
            byte_order: ByteOrder::Little,
        },
        None => {
            return Err(XmlError::Invalid(format!(
                "Boolean node {name} requires explicit bitfield metadata"
            )))
        }
    };
    let (selectors, selected_if) = selector_state.into_parts();

    Ok(NodeDecl::Boolean {
        name,
        addressing,
        len,
        access,
        bitfield,
        selectors,
        selected_if,
    })
}

fn parse_command(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut address = None;
    let mut length = None;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" => {
                    let text = read_text_start(reader, e)?;
                    address = Some(parse_u64(&text)?);
                }
                b"Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    length = Some(u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("length out of range for node {name}"))
                    })?);
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Command node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let address = address
        .ok_or_else(|| XmlError::Invalid(format!("Command node {name} is missing <Address>")))?;
    let length = length.unwrap_or(1);

    Ok(NodeDecl::Command {
        name,
        address,
        len: length,
    })
}

fn parse_command_empty(start: &BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    let address = attribute_value_required(start, b"Address")?;
    let address = parse_u64(&address)?;
    let length = attribute_value(start, b"Length")?;
    let length = match length {
        Some(value) => {
            let raw = parse_u64(&value)?;
            u32::try_from(raw)
                .map_err(|_| XmlError::Invalid("command length out of range".into()))?
        }
        None => 1,
    };
    Ok(NodeDecl::Command {
        name,
        address,
        len: length,
    })
}

fn parse_category(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let node_name = start.name().as_ref().to_vec();
    let mut children = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"pFeature" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        children.push(trimmed.to_string());
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) if e.name().as_ref() == b"pFeature" => {
                if let Some(value) = attribute_value(e, b"Name")? {
                    if !value.is_empty() {
                        children.push(value);
                    }
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Category node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(NodeDecl::Category { name, children })
}

fn parse_category_empty(start: &BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    Ok(NodeDecl::Category {
        name,
        children: Vec::new(),
    })
}

fn parse_swissknife(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut expr: Option<String> = None;
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut output = SkOutput::Float;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Expression" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression>"
                        )));
                    }
                    expr = Some(trimmed.to_string());
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <pVariable>"
                        )));
                    }
                    variables.push((var_name, target.to_string()));
                }
                b"Output" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(kind) = SkOutput::parse(&text) {
                        output = kind;
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    if let Some(target) = attribute_value(e, TAG_VALUE)? {
                        if target.is_empty() {
                            return Err(XmlError::Invalid(format!(
                                "SwissKnife node {name} has empty <pVariable/>"
                            )));
                        }
                        variables.push((var_name, target));
                    } else {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} missing variable target"
                        )));
                    }
                }
                b"Expression" => {
                    let text = attribute_value_required(e, TAG_VALUE)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression/>"
                        )));
                    }
                    expr = Some(trimmed.to_string());
                }
                b"Output" => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        if let Some(kind) = SkOutput::parse(&value) {
                            output = kind;
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated SwissKnife node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let expr = expr.ok_or_else(|| {
        XmlError::Invalid(format!("SwissKnife node {name} is missing <Expression>"))
    })?;
    if variables.is_empty() {
        return Err(XmlError::Invalid(format!(
            "SwissKnife node {name} must declare at least one <pVariable>"
        )));
    }

    Ok(NodeDecl::SwissKnife(SwissKnifeDecl {
        name,
        expr,
        variables,
        output,
    }))
}

fn parse_enum_entry(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<EnumEntryDecl, XmlError> {
    let mut name = attribute_value_required(&start, b"Name")?;
    let mut literal = attribute_value(&start, TAG_VALUE)?;
    let mut provider = attribute_value(&start, TAG_P_VALUE)?;
    let mut display_name = attribute_value(&start, TAG_DISPLAY_NAME)?;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                TAG_VALUE => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        literal = Some(trimmed.to_string());
                    }
                }
                TAG_P_VALUE => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        provider = Some(trimmed.to_string());
                    }
                }
                TAG_DISPLAY_NAME => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        display_name = Some(trimmed.to_string());
                    }
                }
                b"Name" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        name = trimmed.to_string();
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid("unterminated EnumEntry element".into()))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    build_enum_entry(name, literal, provider, display_name)
}

fn parse_enum_entry_empty(start: &BytesStart<'_>) -> Result<EnumEntryDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    let literal = attribute_value(start, TAG_VALUE)?;
    let provider = attribute_value(start, TAG_P_VALUE)?;
    let display_name = attribute_value(start, TAG_DISPLAY_NAME)?;
    build_enum_entry(name, literal, provider, display_name)
}

fn build_enum_entry(
    name: String,
    literal: Option<String>,
    provider: Option<String>,
    display_name: Option<String>,
) -> Result<EnumEntryDecl, XmlError> {
    let literal = literal.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let provider = provider.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    if literal.is_some() && provider.is_some() {
        warn!(
            entry = %name,
            "EnumEntry specifies both <Value> and <pValue>; preferring provider"
        );
    }

    let value = if let Some(node) = provider {
        EnumValueSrc::FromNode(node)
    } else if let Some(value) = literal {
        EnumValueSrc::Literal(parse_i64(&value)?)
    } else {
        return Err(XmlError::Invalid(format!(
            "EnumEntry {name} is missing <Value> or <pValue>"
        )));
    };

    Ok(EnumEntryDecl {
        name,
        value,
        display_name,
    })
}

fn parse_scale(text: &str) -> Result<(i64, i64), XmlError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(XmlError::Invalid("empty scale value".into()));
    }
    if let Some((num, den)) = trimmed.split_once('/') {
        let num = parse_i64(num)?;
        let den = parse_i64(den)?;
        if den == 0 {
            return Err(XmlError::Invalid("scale denominator is zero".into()));
        }
        Ok((num, den))
    } else {
        let value = parse_f64(trimmed)?;
        if value == 0.0 {
            return Err(XmlError::Invalid("scale value is zero".into()));
        }
        // Approximate decimal scale as a rational using denominator 1_000_000.
        let den = 1_000_000i64;
        let num = (value * den as f64).round() as i64;
        Ok((num, den))
    }
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
        if let Some(name) = attribute_value(event, b"Name")? {
            top_level.push(name);
        } else {
            top_level.push(String::from_utf8_lossy(event.name().as_ref()).to_string());
        }
    }
    Ok(())
}

fn extract_schema_version(event: &BytesStart<'_>) -> Option<String> {
    let major = attribute_value(event, b"SchemaMajorVersion").ok().flatten();
    let minor = attribute_value(event, b"SchemaMinorVersion").ok().flatten();
    let sub = attribute_value(event, b"SchemaSubMinorVersion")
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

fn read_text_start(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<String, XmlError> {
    let end_buf = start.name().as_ref().to_vec();
    reader
        .read_text(QName(&end_buf))
        .map(|cow| cow.into_owned())
        .map_err(|err| XmlError::Xml(err.to_string()))
}

fn attribute_value(event: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>, XmlError> {
    for attr in event.attributes() {
        let attr = attr.map_err(|err| XmlError::Xml(err.to_string()))?;
        if attr.key.as_ref() == name {
            let value = attr
                .unescape_value()
                .map_err(|err| XmlError::Xml(err.to_string()))?;
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                return Ok(None);
            }
            return Ok(Some(trimmed));
        }
    }
    Ok(None)
}

fn attribute_value_required(event: &BytesStart<'_>, name: &[u8]) -> Result<String, XmlError> {
    attribute_value(event, name)?.ok_or_else(|| {
        XmlError::Invalid(format!(
            "missing attribute {}",
            String::from_utf8_lossy(name)
        ))
    })
}

fn parse_u64(value: &str) -> Result<u64, XmlError> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let hex = hex.replace('_', "");
        u64::from_str_radix(&hex, 16)
            .map_err(|err| XmlError::Invalid(format!("invalid hex value: {err}")))
    } else {
        let dec = trimmed.replace('_', "");
        dec.parse()
            .map_err(|err| XmlError::Invalid(format!("invalid integer: {err}")))
    }
}

fn parse_i64(value: &str) -> Result<i64, XmlError> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let hex = hex.replace('_', "");
        i64::from_str_radix(&hex, 16)
            .map_err(|err| XmlError::Invalid(format!("invalid hex value: {err}")))
    } else {
        let dec = trimmed.replace('_', "");
        dec.parse()
            .map_err(|err| XmlError::Invalid(format!("invalid integer: {err}")))
    }
}

fn parse_f64(value: &str) -> Result<f64, XmlError> {
    value
        .trim()
        .parse()
        .map_err(|err| XmlError::Invalid(format!("invalid float: {err}")))
}

fn skip_element(reader: &mut Reader<&[u8]>, name: &[u8]) -> Result<(), XmlError> {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    while depth > 0 {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == name {
                    depth -= 1;
                }
            }
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid("unexpected end of file".into()));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimalXmlInfo {
    pub schema_version: Option<String>,
    pub top_level_features: Vec<String>,
}

fn first_cstring(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    let value = String::from_utf8_lossy(slice).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[derive(Debug)]
enum UrlLocation {
    Local { address: u64, length: usize },
    LocalNamed(String),
    Http(String),
    File(String),
}

impl UrlLocation {
    fn parse(url: &str) -> Result<Self, XmlError> {
        if let Some(rest) = url.strip_prefix("local:") {
            parse_local_url(rest)
        } else if url.starts_with("http://") || url.starts_with("https://") {
            Ok(UrlLocation::Http(url.to_string()))
        } else if url.starts_with("file://") {
            Ok(UrlLocation::File(url.to_string()))
        } else {
            Err(XmlError::Unsupported(format!("unknown URL scheme: {url}")))
        }
    }
}

fn parse_local_url(rest: &str) -> Result<UrlLocation, XmlError> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return Err(XmlError::Invalid("empty local URL".into()));
    }
    let mut address = None;
    let mut length = None;
    for part in trimmed.split([';', ',']) {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((key, value)) = token.split_once('=') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "address" | "addr" | "offset" => {
                    address = Some(parse_u64(value)?);
                }
                "length" | "size" => {
                    let len = parse_u64(value)?;
                    length = Some(
                        len.try_into()
                            .map_err(|_| XmlError::Invalid("length does not fit usize".into()))?,
                    );
                }
                _ => {}
            }
        } else if token.starts_with("0x") {
            address = Some(parse_u64(token)?);
        } else {
            return Ok(UrlLocation::LocalNamed(token.to_string()));
        }
    }
    match (address, length) {
        (Some(address), Some(length)) => Ok(UrlLocation::Local { address, length }),
        _ => Err(XmlError::Invalid(format!("unsupported local URL: {rest}"))),
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

    #[tokio::test]
    async fn parse_minimal_xml() {
        let info = parse_into_minimal_nodes(FIXTURE).expect("parse xml");
        assert_eq!(info.schema_version.as_deref(), Some("1.2.3"));
        assert_eq!(info.top_level_features.len(), 7);
        assert_eq!(info.top_level_features[0], "Root");

        let data = b"local:address=0x10;length=0x3\0".to_vec();
        let xml_payload = b"<a/>".to_vec();
        let loaded = fetch_and_load_xml(|addr, len| {
            let data = data.clone();
            let xml_payload = xml_payload.clone();
            async move {
                if addr == FIRST_URL_ADDRESS {
                    Ok(data)
                } else if addr == 0x10 && len == 0x3 {
                    Ok(xml_payload)
                } else {
                    Err(XmlError::Transport("unexpected read".into()))
                }
            }
        })
        .await
        .expect("load xml");
        assert_eq!(loaded, "<a/>");
    }

    #[test]
    fn parse_fixture_model() {
        let model = parse(FIXTURE).expect("parse fixture");
        assert_eq!(model.version, "1.2.3");
        assert_eq!(model.nodes.len(), 7);
        match &model.nodes[0] {
            NodeDecl::Category { name, children } => {
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

    #[test]
    fn parse_indirect_addressing() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegAddr">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                </Integer>
                <Integer Name="Gain" Address="0xFFFF">
                    <pAddress>RegAddr</pAddress>
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
                assert!(
                    matches!(addressing, Addressing::Fixed { address, len } if *address == 0x2000 && *len == 4)
                );
            }
            other => panic!("unexpected node: {other:?}"),
        }
        match &model.nodes[1] {
            NodeDecl::Integer {
                name, addressing, ..
            } => {
                assert_eq!(name, "Gain");
                match addressing {
                    Addressing::Indirect {
                        p_address_node,
                        len,
                    } => {
                        assert_eq!(p_address_node, "RegAddr");
                        assert_eq!(*len, 4);
                    }
                    other => panic!("expected indirect addressing, got {other:?}"),
                }
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn parse_integer_bitfield_big_endian() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="Packed">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                    <Lsb>8</Lsb>
                    <Msb>15</Msb>
                    <Endianness>BigEndian</Endianness>
                </Integer>
            </RegisterDescription>
        "#;

        let model = parse(XML).expect("parse big-endian bitfield");
        assert_eq!(model.nodes.len(), 1);
        match &model.nodes[0] {
            NodeDecl::Integer { len, bitfield, .. } => {
                assert_eq!(*len, 4);
                let field = bitfield.expect("bitfield present");
                assert_eq!(field.byte_order, ByteOrder::Big);
                assert_eq!(field.bit_length, 8);
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
                assert_eq!(bitfield.byte_order, ByteOrder::Little);
                assert_eq!(bitfield.bit_length, 1);
                assert_eq!(bitfield.bit_offset, 3);
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
                let field = bitfield.expect("bitfield present");
                assert_eq!(field.byte_order, ByteOrder::Little);
                assert_eq!(field.bit_length, 8);
                assert_eq!(field.bit_offset, 8);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }
}
