//! Parsers for Enumeration and Boolean nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use tracing::warn;

use super::{
    NodeMetaBuilder, TAG_BIT, TAG_BYTE_ORDER, TAG_DISPLAY_NAME, TAG_ENDIANESS, TAG_ENDIANNESS,
    TAG_INT_SWISS_KNIFE, TAG_LSB, TAG_LSB_MIXED, TAG_MASK, TAG_MSB, TAG_MSB_MIXED, TAG_P_ADDRESS,
    TAG_P_INDEX, TAG_P_PORT, TAG_P_SELECTED, TAG_P_VALUE, TAG_VALUE, handle_addressing_empty,
    handle_addressing_start, handle_p_selected_start, handle_predicate_start,
};
use crate::builders::{AddressingBuilder, BitfieldBuilder};
use crate::util::{
    attribute_value, attribute_value_required, parse_address, parse_i64, parse_u64, read_text_start,
};
use crate::{
    AccessMode, BitField, ByteOrder, EnumEntryDecl, EnumValueSrc, NodeDecl, PredicateRefs, XmlError,
};

/// Parse an `<Enumeration>` element into a [`NodeDecl::Enum`].
pub(crate) fn parse_enum(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, "Address")? {
        addressing.push_fixed_address(parse_address(&addr)?);
    }
    if let Some(len) = attribute_value(&start, "Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::invalid(&name, "declares a <Length> out of range"))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut entries = Vec::new();
    let mut default = None;
    let mut predicates = PredicateRefs::default();
    let mut selected = Vec::new();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    let mut pvalue = None;
    let mut value = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                // The enumeration's own `<Value>`; an entry's is read with
                // the entry.
                TAG_VALUE => {
                    let text = read_text_start(reader, e)?;
                    value = Some(parse_i64(&text)?);
                }
                "Address" | TAG_P_ADDRESS | TAG_P_INDEX | TAG_INT_SWISS_KNIFE | TAG_P_PORT
                | "Length" => {
                    if !handle_addressing_start(reader, e, &name, &mut addressing)? {
                        meta_builder.ignore(reader, e, None)?;
                    }
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                "EnumEntry" => {
                    let entry = parse_enum_entry(reader, e.clone(), &mut meta_builder)?;
                    entries.push(entry);
                }
                TAG_P_SELECTED => {
                    handle_p_selected_start(reader, e, &mut selected)?;
                }
                "pValueDefault" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        default = Some(trimmed.to_string());
                    }
                }
                _ => {
                    if handle_predicate_start(reader, e, &mut predicates)? {
                        // handled
                    } else {
                        meta_builder.handle_or_ignore(reader, e)?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => {
                if e.name().as_ref() == "EnumEntry" {
                    entries.push(parse_enum_entry_empty(e)?);
                } else {
                    meta_builder.ignore_empty(e);
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    if entries.is_empty() {
        return Err(XmlError::missing(&name, "<EnumEntry> elements"));
    }

    // Addressing is optional: nodes may delegate via pValue, or may exist
    // as pure selectors without direct register backing.
    let addressing = addressing.finalize(&name, Some(4)).ok();

    Ok(NodeDecl::Enum {
        name,
        meta: meta_builder.build(),
        addressing,
        access,
        entries,
        default,
        selected,
        pvalue,
        value,
        predicates,
    })
}

/// Parse a `<Boolean>` element into a [`NodeDecl::Boolean`].
pub(crate) fn parse_boolean(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut addressing = AddressingBuilder::default();
    if let Some(addr) = attribute_value(&start, "Address")? {
        addressing.push_fixed_address(parse_address(&addr)?);
    }
    if let Some(len) = attribute_value(&start, "Length")? {
        let value = parse_u64(&len)?;
        let len = u32::try_from(value)
            .map_err(|_| XmlError::invalid(&name, "declares a <Length> out of range"))?;
        addressing.set_length(len);
    }
    let mut access = AccessMode::RW;
    let mut pvalue = None;
    let mut on_value = None;
    let mut off_value = None;
    let mut value = None;
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();
    let mut bitfield = BitfieldBuilder::default();
    let mut pending_bit_length = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                "OnValue" => {
                    let text = read_text_start(reader, e)?;
                    on_value = Some(parse_i64(&text)?);
                }
                "OffValue" => {
                    let text = read_text_start(reader, e)?;
                    off_value = Some(parse_i64(&text)?);
                }
                TAG_VALUE => {
                    let text = read_text_start(reader, e)?;
                    value = Some(parse_i64(&text)?);
                }
                // Shared handling so `<Address>`, `<pAddress>` and
                // `<pIndex>` all contribute their term.
                "Address" | TAG_P_ADDRESS | TAG_P_INDEX | TAG_INT_SWISS_KNIFE | TAG_P_PORT => {
                    handle_addressing_start(reader, e, &name, &mut addressing)?;
                }
                "Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let mut handled = false;
                    if pending_bit_length {
                        if let Ok(bit_len) = u32::try_from(value) {
                            bitfield.note_bit_length(bit_len);
                            pending_bit_length = false;
                            handled = true;
                        } else {
                            return Err(XmlError::invalid(
                                &name,
                                "declares a bit field length out of range",
                            ));
                        }
                    }
                    if !handled {
                        let len = u32::try_from(value).map_err(|_| {
                            XmlError::invalid(&name, "declares a <Length> out of range")
                        })?;
                        addressing.set_length(len);
                    }
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                TAG_LSB | TAG_LSB_MIXED => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let lsb = u32::try_from(value)
                        .map_err(|_| XmlError::invalid(&name, "declares an <LSB> out of range"))?;
                    bitfield.note_lsb(lsb);
                }
                TAG_MSB | TAG_MSB_MIXED => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let msb = u32::try_from(value)
                        .map_err(|_| XmlError::invalid(&name, "declares an <MSB> out of range"))?;
                    bitfield.note_msb(msb);
                }
                TAG_BIT => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    let bit = u32::try_from(value)
                        .map_err(|_| XmlError::invalid(&name, "declares a <Bit> out of range"))?;
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
                _ => {
                    if handle_predicate_start(reader, e, &mut predicates)? {
                        // handled
                    } else {
                        meta_builder.handle_or_ignore(reader, e)?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_ADDRESS => {
                    handle_addressing_empty(e, &mut addressing)?;
                }
                TAG_LSB | TAG_LSB_MIXED => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let lsb = u32::try_from(parsed).map_err(|_| {
                            XmlError::invalid(&name, "declares an <LSB> out of range")
                        })?;
                        bitfield.note_lsb(lsb);
                    }
                }
                TAG_MSB | TAG_MSB_MIXED => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let msb = u32::try_from(parsed).map_err(|_| {
                            XmlError::invalid(&name, "declares an <MSB> out of range")
                        })?;
                        bitfield.note_msb(msb);
                    }
                }
                TAG_BIT => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)? {
                        let parsed = parse_u64(&value)?;
                        let bit = u32::try_from(parsed).map_err(|_| {
                            XmlError::invalid(&name, "declares a <Bit> out of range")
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
                    if let Some(value) = attribute_value(e, TAG_VALUE)?
                        && let Some(order) = ByteOrder::parse(&value)
                    {
                        bitfield.note_byte_order(order);
                    }
                }
                _ => meta_builder.ignore_empty(e),
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    // Addressing is optional: nodes may delegate via pValue or appear as
    // pure UI features without register backing.
    let (addressing, len, bitfield) = if let Ok(addr) = addressing.finalize(&name, Some(4)) {
        let len = addr.byte_len();
        let bf = match bitfield.finish(&name, len)? {
            Some(field) => Some(field),
            None if len == 1 => Some(BitField {
                bit_offset: 0,
                bit_length: 1,
                byte_order: ByteOrder::Little,
            }),
            None => {
                return Err(XmlError::missing(
                    &name,
                    "a bit field (<Bit>, <LSB>/<MSB> or <Mask>)",
                ));
            }
        };
        (Some(addr), len, bf)
    } else {
        // No register backing (pValue delegation).
        (None, 4, None)
    };

    Ok(NodeDecl::Boolean {
        name,
        meta: meta_builder.build(),
        addressing,
        len,
        access,
        bitfield,
        pvalue,
        on_value,
        off_value,
        value,
        predicates,
    })
}

/// Parse an `<EnumEntry>` start element. A child it does not interpret is
/// recorded on the enumeration, as `EnumEntry/<tag>`.
fn parse_enum_entry(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
    enumeration: &mut NodeMetaBuilder,
) -> Result<EnumEntryDecl, XmlError> {
    let mut name = attribute_value_required(&start, "Name")?;
    let mut literal = attribute_value(&start, TAG_VALUE)?;
    let mut provider = attribute_value(&start, TAG_P_VALUE)?;
    let mut display_name = attribute_value(&start, TAG_DISPLAY_NAME)?;
    let mut predicates = PredicateRefs::default();
    let mut is_self_clearing = false;
    let node_name = start.name().as_ref().to_string();
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
                "Name" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        name = trimmed.to_string();
                    }
                }
                "IsSelfClearing" => {
                    let text = read_text_start(reader, e)?;
                    is_self_clearing = text.trim() == "Yes";
                }
                _ => {
                    if !handle_predicate_start(reader, e, &mut predicates)? {
                        enumeration.ignore(reader, e, Some("EnumEntry"))?;
                    }
                }
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(
                    "<EnumEntry>",
                    "ends before its closing tag",
                ));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    let mut entry = build_enum_entry(name, literal, provider, display_name, predicates)?;
    entry.is_self_clearing = is_self_clearing;
    Ok(entry)
}

/// Parse an empty `<EnumEntry />` element.
fn parse_enum_entry_empty(start: &BytesStart<'_>) -> Result<EnumEntryDecl, XmlError> {
    let name = attribute_value_required(start, "Name")?;
    let literal = attribute_value(start, TAG_VALUE)?;
    let provider = attribute_value(start, TAG_P_VALUE)?;
    let display_name = attribute_value(start, TAG_DISPLAY_NAME)?;
    build_enum_entry(
        name,
        literal,
        provider,
        display_name,
        PredicateRefs::default(),
    )
}

/// Build an [`EnumEntryDecl`] from parsed components.
fn build_enum_entry(
    name: String,
    literal: Option<String>,
    provider: Option<String>,
    display_name: Option<String>,
    predicates: PredicateRefs,
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
        return Err(XmlError::missing(&name, "<Value> or <pValue>"));
    };

    Ok(EnumEntryDecl {
        name,
        value,
        display_name,
        predicates,
        is_self_clearing: false,
    })
}
