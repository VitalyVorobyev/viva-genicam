//! Node-specific parsers and shared parsing helpers.

mod control;
mod converter;
mod numeric;
mod struct_reg;
mod swissknife;
mod symbolic;

pub use control::{parse_category, parse_category_empty, parse_command, parse_command_empty};
pub use converter::{parse_converter, parse_int_converter, parse_string};
pub use numeric::{parse_float, parse_integer};
pub use struct_reg::parse_struct_reg;
pub use swissknife::parse_swissknife;
pub use symbolic::{parse_boolean, parse_enum};

use quick_xml::Reader;
use quick_xml::events::BytesStart;

use crate::builders::AddressingBuilder;
use crate::util::{attribute_value, parse_u64, read_text_start};
use crate::{NodeMeta, Representation, Visibility, XmlError};

/// XML element name referencing another node that provides an address.
pub const TAG_P_ADDRESS: &[u8] = b"pAddress";
/// XML element holding an inline literal value.
pub const TAG_VALUE: &[u8] = b"Value";
/// XML element referencing another node supplying the value at runtime.
pub const TAG_P_VALUE: &[u8] = b"pValue";
/// XML element specifying a user friendly label.
pub const TAG_DISPLAY_NAME: &[u8] = b"DisplayName";
/// XML element for node visibility level.
pub const TAG_VISIBILITY: &[u8] = b"Visibility";
/// XML element for the long-form description.
pub const TAG_DESCRIPTION: &[u8] = b"Description";
/// XML element for short tooltip text.
pub const TAG_TOOLTIP: &[u8] = b"ToolTip";
/// XML element for the recommended numeric representation.
pub const TAG_REPRESENTATION: &[u8] = b"Representation";
/// XML element describing the least significant bit of a bitfield.
pub const TAG_LSB: &[u8] = b"Lsb";
/// XML element describing the most significant bit of a bitfield.
pub const TAG_MSB: &[u8] = b"Msb";
/// XML element describing the starting bit index of a bitfield.
pub const TAG_BIT: &[u8] = b"Bit";
/// XML element describing a bitmask for a bitfield.
pub const TAG_MASK: &[u8] = b"Mask";
/// XML element providing the register byte order (common spelling).
pub const TAG_ENDIANNESS: &[u8] = b"Endianness";
/// XML element providing the register byte order (alternate spelling).
pub const TAG_ENDIANESS: &[u8] = b"Endianess";
/// XML element providing the register byte order (PFNC style).
pub const TAG_BYTE_ORDER: &[u8] = b"ByteOrder";

/// Tracks selector state during node parsing.
///
/// This struct consolidates the `selectors`, `selected_if`, and `last_selector`
/// variables that were previously duplicated across all node parsers.
#[derive(Debug, Default)]
pub struct SelectorState {
    /// List of selector node names referencing this feature.
    pub selectors: Vec<String>,
    /// Selector gating rules in the form (selector name, allowed values).
    pub selected_if: Vec<(String, Vec<String>)>,
    /// Index into `selected_if` for the most recent pSelected element.
    pub last_selector: Option<usize>,
}

impl SelectorState {
    /// Finalize into the component parts for NodeDecl construction.
    pub fn into_parts(self) -> (Vec<String>, Vec<(String, Vec<String>)>) {
        (self.selectors, self.selected_if)
    }
}

/// Handle a `<pSelected>` start element.
///
/// Reads the text content, registers the selector with addressing, and updates selector state.
pub fn handle_p_selected_start(
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
pub fn handle_selected_start(
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
pub fn handle_p_selected_empty(
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
pub fn handle_selected_empty(
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
pub fn handle_addressing_start(
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

/// Accumulates metadata fields during node parsing.
#[derive(Debug, Default)]
pub struct NodeMetaBuilder {
    pub visibility: Option<Visibility>,
    pub description: Option<String>,
    pub tooltip: Option<String>,
    pub display_name: Option<String>,
    pub representation: Option<Representation>,
}

impl NodeMetaBuilder {
    /// Try to handle a start element as a metadata tag.
    ///
    /// Returns `true` if the tag was consumed, `false` otherwise.
    pub fn handle_start(
        &mut self,
        reader: &mut Reader<&[u8]>,
        event: &BytesStart<'_>,
    ) -> Result<bool, XmlError> {
        match event.name().as_ref() {
            TAG_VISIBILITY => {
                let text = read_text_start(reader, event)?;
                self.visibility = Visibility::parse(&text);
                Ok(true)
            }
            TAG_DESCRIPTION => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.description = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_TOOLTIP => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.tooltip = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_DISPLAY_NAME => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.display_name = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_REPRESENTATION => {
                let text = read_text_start(reader, event)?;
                self.representation = Representation::parse(&text);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Build the final [`NodeMeta`].
    pub fn build(self) -> NodeMeta {
        NodeMeta {
            visibility: self.visibility.unwrap_or_default(),
            description: self.description,
            tooltip: self.tooltip,
            display_name: self.display_name,
            representation: self.representation,
        }
    }
}

/// Handle common addressing elements for empty events.
///
/// Returns `true` if the element was handled, `false` otherwise.
pub fn handle_addressing_empty(
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
