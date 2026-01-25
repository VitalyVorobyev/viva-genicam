//! Parsers for Enumeration and Boolean nodes.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use tracing::warn;

use super::{
    handle_addressing_empty, handle_addressing_start, handle_p_selected_empty,
    handle_p_selected_start, handle_selected_empty, handle_selected_start, SelectorState, TAG_BIT,
    TAG_BYTE_ORDER, TAG_DISPLAY_NAME, TAG_ENDIANESS, TAG_ENDIANNESS, TAG_LSB, TAG_MASK, TAG_MSB,
    TAG_P_ADDRESS, TAG_P_VALUE, TAG_VALUE,
};
use crate::builders::{addressing_lengths, AddressingBuilder, BitfieldBuilder};
use crate::util::{
    attribute_value, attribute_value_required, parse_i64, parse_u64, read_text_start, skip_element,
};
use crate::{AccessMode, BitField, ByteOrder, EnumEntryDecl, EnumValueSrc, NodeDecl, XmlError};

/// Parse an `<Enumeration>` element into a [`NodeDecl::Enum`].
pub fn parse_enum(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
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

/// Parse a `<Boolean>` element into a [`NodeDecl::Boolean`].
pub fn parse_boolean(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
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

/// Parse an `<EnumEntry>` start element.
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

/// Parse an empty `<EnumEntry />` element.
fn parse_enum_entry_empty(start: &BytesStart<'_>) -> Result<EnumEntryDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    let literal = attribute_value(start, TAG_VALUE)?;
    let provider = attribute_value(start, TAG_P_VALUE)?;
    let display_name = attribute_value(start, TAG_DISPLAY_NAME)?;
    build_enum_entry(name, literal, provider, display_name)
}

/// Build an [`EnumEntryDecl`] from parsed components.
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
