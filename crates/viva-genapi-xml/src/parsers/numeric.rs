//! Parsers for Integer and Float nodes.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{
    handle_addressing_empty, handle_addressing_start, handle_p_selected_empty,
    handle_p_selected_start, handle_selected_empty, handle_selected_start, SelectorState, TAG_BIT,
    TAG_BYTE_ORDER, TAG_ENDIANESS, TAG_ENDIANNESS, TAG_LSB, TAG_MASK, TAG_MSB, TAG_P_ADDRESS,
    TAG_VALUE,
};
use crate::builders::{addressing_lengths, AddressingBuilder, BitfieldBuilder};
use crate::util::{
    attribute_value, attribute_value_required, parse_f64, parse_i64, parse_scale, parse_u64,
    read_text_start, skip_element,
};
use crate::{AccessMode, ByteOrder, NodeDecl, XmlError};

/// Parse an `<Integer>` element into a [`NodeDecl::Integer`].
pub fn parse_integer(
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
    let mut min = None;
    let mut max = None;
    let mut inc = None;
    let mut unit = None;
    let mut pvalue = None;
    let mut p_max = None;
    let mut p_min = None;
    let mut static_value: Option<i64> = None;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut bitfield = BitfieldBuilder::default();
    let mut pending_bit_length = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                b"pMax" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_max = Some(target.to_string());
                    }
                }
                b"pMin" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_min = Some(target.to_string());
                    }
                }
                TAG_VALUE => {
                    let text = read_text_start(reader, e)?;
                    static_value = Some(parse_i64(&text)?);
                }
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

    // Min/Max are optional per GenICam standard; use full-range defaults.
    let min = min.unwrap_or(i64::MIN);
    let max = max.unwrap_or(i64::MAX);

    // When pValue or static Value is set, addressing is optional.
    let (addressing, len, bitfield) = if pvalue.is_some() || static_value.is_some() {
        let addr = addressing.finalize(&name, Some(4)).ok();
        let len = addr
            .as_ref()
            .and_then(|a| addressing_lengths(a).first().copied())
            .unwrap_or(4);
        let lengths = addr.as_ref().map(addressing_lengths).unwrap_or_default();
        let bf = bitfield.finish(&name, &lengths).ok().flatten();
        (addr, len, bf)
    } else {
        let addr = addressing.finalize(&name, Some(4))?;
        let lengths = addressing_lengths(&addr);
        let len = lengths
            .first()
            .copied()
            .ok_or_else(|| XmlError::Invalid(format!("node {name} is missing <Length>")))?;
        let bf = bitfield.finish(&name, &lengths)?;
        (Some(addr), len, bf)
    };
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
        pvalue,
        p_max,
        p_min,
        value: static_value,
    })
}

/// Parse a `<Float>` element into a [`NodeDecl::Float`].
pub fn parse_float(
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
    let mut min = None;
    let mut max = None;
    let mut unit = None;
    let mut scale_num: Option<i64> = None;
    let mut scale_den: Option<i64> = None;
    let mut offset = None;
    let mut pvalue = None;
    let mut selector_state = SelectorState::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
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

    let min = min.unwrap_or(f64::MIN);
    let max = max.unwrap_or(f64::MAX);
    let scale = match (scale_num, scale_den) {
        (Some(num), Some(den)) if den != 0 => Some((num, den)),
        (None, None) => None,
        (Some(num), None) => Some((num, 1)),
        _ => None,
    };

    let addressing = if pvalue.is_some() {
        addressing.finalize(&name, Some(8)).ok()
    } else {
        Some(addressing.finalize(&name, Some(8))?)
    };
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
        pvalue,
    })
}
