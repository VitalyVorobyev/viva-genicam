//! Parsers for Integer and Float nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{
    IndexedBuilder, NodeMetaBuilder, TAG_BIT, TAG_BYTE_ORDER, TAG_ENDIANESS, TAG_ENDIANNESS,
    TAG_INT_SWISS_KNIFE, TAG_LSB, TAG_LSB_MIXED, TAG_MASK, TAG_MSB, TAG_MSB_MIXED, TAG_P_ADDRESS,
    TAG_P_INDEX, TAG_P_LENGTH, TAG_P_PORT, TAG_P_SELECTED, TAG_VALUE, handle_addressing_empty,
    handle_addressing_start, handle_invalidator_start, handle_p_selected_start,
    handle_predicate_start,
};
use crate::builders::{AddressingBuilder, BitfieldBuilder};
use crate::util::{
    attribute_value, attribute_value_required, parse_address, parse_f64, parse_i64, parse_scale,
    parse_u64, read_text_start,
};
use crate::{AccessMode, ByteOrder, FloatEncoding, NodeDecl, PredicateRefs, Sign, XmlError};

/// Parse an `<Integer>` element into a [`NodeDecl::Integer`].
pub(crate) fn parse_integer(
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
    let mut sign = Sign::default();
    let mut byte_order: Option<ByteOrder> = None;
    let mut min = None;
    let mut max = None;
    let mut inc = None;
    let mut unit = None;
    let mut pvalue = None;
    let mut p_max = None;
    let mut p_min = None;
    let mut p_inc = None;
    let mut static_value: Option<i64> = None;
    let mut predicates = PredicateRefs::default();
    let mut invalidators = Vec::new();
    let mut selected = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();
    let node_name = start.name().as_ref().to_string();
    // An `<IntReg>` or a `<MaskedIntReg>` is a register; an `<Integer>` is
    // not, and its `<pIndex>` selects a value (GenICam Standard v2.1.1
    // §2.8.5).
    let is_register = node_name != "Integer";
    let mut indexed = IndexedBuilder::default();
    let mut buf = Vec::new();
    let mut bitfield = BitfieldBuilder::default();
    let mut pending_bit_length = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e))
                if !is_register && indexed.handle_start(reader, e, parse_i64)? => {}
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                "pMax" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_max = Some(target.to_string());
                    }
                }
                "pMin" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_min = Some(target.to_string());
                    }
                }
                "pInc" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_inc = Some(target.to_string());
                    }
                }
                TAG_VALUE => {
                    let text = read_text_start(reader, e)?;
                    static_value = Some(parse_i64(&text)?);
                }
                // Shared handling so `<Address>`, `<pAddress>` and
                // `<pIndex>` all contribute their term, and `<pLength>` its
                // node (GenICam Standard v2.1.1 §2.8.3) — the Lucid
                // TRT009S's event frame IDs are 2 or 8 bytes by it.
                "Address" | TAG_P_ADDRESS | TAG_P_INDEX | TAG_INT_SWISS_KNIFE | TAG_P_PORT
                | TAG_P_LENGTH => {
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
                "Sign" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(parsed) = Sign::parse(&text) {
                        sign = parsed;
                    }
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                "Min" => {
                    let text = read_text_start(reader, e)?;
                    min = Some(parse_i64(&text)?);
                }
                "Max" => {
                    let text = read_text_start(reader, e)?;
                    max = Some(parse_i64(&text)?);
                }
                "Inc" => {
                    let text = read_text_start(reader, e)?;
                    inc = Some(parse_i64(&text)?);
                }
                "Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
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
                        // Both: the bitfield needs it to place the bits, and an
                        // unmasked register has no bitfield to hold it at all.
                        bitfield.note_byte_order(order);
                        byte_order = Some(order);
                    }
                }
                TAG_P_SELECTED => {
                    handle_p_selected_start(reader, e, &mut selected)?;
                }
                _ => {
                    if handle_predicate_start(reader, e, &mut predicates)?
                        || handle_invalidator_start(reader, e, &mut invalidators)?
                    {
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
                        byte_order = Some(order);
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

    let indexed = indexed.finish(&name, &mut addressing)?;
    // Addressing is optional: nodes may delegate via pValue, have a static
    // Value, or appear as pure UI features without register backing.
    let (addressing, len, bitfield) = if let Ok(addr) = addressing.finalize(&name, Some(4)) {
        let len = addr.byte_len();
        // A bit field is placed within a length known now; one a
        // `<pLength>` node gives later is not, and a field placed against
        // the `0` that stands for it would be refused as a zero-length
        // register, which says the wrong thing.
        if len == 0 && addr.length_node().is_some() && bitfield.is_declared() {
            return Err(XmlError::invalid(
                &name,
                "declares a bit field on a register whose length only its <pLength> gives",
            ));
        }
        let bf = bitfield.finish(&name, len)?;
        (Some(addr), len, bf)
    } else {
        // No register backing (pValue delegation, static Value, etc.)
        (None, 4, bitfield.finish(&name, 4).ok().flatten())
    };

    Ok(NodeDecl::Integer {
        name,
        meta: meta_builder.build(),
        addressing,
        len,
        access,
        min,
        max,
        inc,
        unit,
        bitfield,
        sign,
        byte_order: byte_order.unwrap_or(ByteOrder::Big),
        selected,
        pvalue,
        p_max,
        p_min,
        p_inc,
        value: static_value,
        indexed,
        predicates,
        invalidators,
    })
}

/// Parse a `<Float>` or `<FloatReg>` element into a [`NodeDecl::Float`].
///
/// `<FloatReg>` forces [`FloatEncoding::Ieee754`]. `<Float>` infers the
/// encoding: IEEE 754 when there is direct addressing with `Length ∈ {4, 8}`
/// and no `<Scale>`/`<Offset>`; otherwise scaled-integer (the historical
/// default, which preserves compatibility with XMLs that rely on scale-only
/// semantics).
pub(crate) fn parse_float(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let node_name = start.name().as_ref().to_string();
    let is_float_reg = node_name.as_str() == "FloatReg";
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
    let mut min = None;
    let mut max = None;
    let mut unit = None;
    let mut scale_num: Option<i64> = None;
    let mut scale_den: Option<i64> = None;
    let mut offset = None;
    let mut pvalue = None;
    let mut p_min = None;
    let mut p_max = None;
    let mut byte_order: Option<ByteOrder> = None;
    let mut value = None;
    let mut indexed = IndexedBuilder::default();
    let mut predicates = PredicateRefs::default();
    let mut invalidators = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            // A `<Float>` is no register: its `<pIndex>` selects a value
            // (GenICam Standard v2.1.1 §2.8.9).
            Ok(Event::Start(ref e))
                if !is_float_reg && indexed.handle_start(reader, e, parse_f64)? => {}
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                "pMin" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_min = Some(target.to_string());
                    }
                }
                "pMax" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_max = Some(target.to_string());
                    }
                }
                "Address" | TAG_P_ADDRESS | TAG_P_INDEX | TAG_INT_SWISS_KNIFE | TAG_P_PORT
                | "Length" => {
                    if !handle_addressing_start(reader, e, &name, &mut addressing)? {
                        meta_builder.ignore(reader, e, None)?;
                    }
                }
                // A floating `<Float>`'s start value (backlog GA-49).
                TAG_VALUE if !is_float_reg => {
                    let text = read_text_start(reader, e)?;
                    value = Some(parse_f64(&text)?);
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                "Min" => {
                    let text = read_text_start(reader, e)?;
                    min = Some(parse_f64(&text)?);
                }
                "Max" => {
                    let text = read_text_start(reader, e)?;
                    max = Some(parse_f64(&text)?);
                }
                "Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                "Scale" => {
                    let text = read_text_start(reader, e)?;
                    let (num, den) = parse_scale(&text)?;
                    scale_num = Some(num);
                    scale_den = Some(den);
                }
                "ScaleNumerator" => {
                    let text = read_text_start(reader, e)?;
                    scale_num = Some(parse_i64(&text)?);
                }
                "ScaleDenominator" => {
                    let text = read_text_start(reader, e)?;
                    scale_den = Some(parse_i64(&text)?);
                }
                "Offset" => {
                    let text = read_text_start(reader, e)?;
                    offset = Some(parse_f64(&text)?);
                }
                TAG_ENDIANNESS | TAG_ENDIANESS | TAG_BYTE_ORDER => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        byte_order = Some(order);
                    }
                }
                _ => {
                    if handle_predicate_start(reader, e, &mut predicates)?
                        || handle_invalidator_start(reader, e, &mut invalidators)?
                    {
                        // handled
                    } else {
                        meta_builder.handle_or_ignore(reader, e)?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => {
                if !handle_addressing_empty(e, &mut addressing)? {
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

    let scale = match (scale_num, scale_den) {
        (Some(num), Some(den)) if den != 0 => Some((num, den)),
        (None, None) => None,
        (Some(num), None) => Some((num, 1)),
        _ => None,
    };

    let indexed = indexed.finish(&name, &mut addressing)?;
    let addressing = addressing.finalize(&name, Some(8)).ok();

    let has_scale_or_offset = scale.is_some() || offset.is_some();
    let length = addressing.as_ref().and_then(addressing_primary_length);
    let native_ieee754 =
        is_float_reg || (!has_scale_or_offset && matches!(length, Some(4) | Some(8)));
    let encoding = if native_ieee754 {
        FloatEncoding::Ieee754
    } else {
        FloatEncoding::ScaledInteger
    };
    let byte_order = byte_order.unwrap_or(ByteOrder::Big);

    Ok(NodeDecl::Float {
        name,
        meta: meta_builder.build(),
        addressing,
        access,
        min,
        max,
        unit,
        scale,
        offset,
        pvalue,
        p_min,
        p_max,
        value,
        indexed,
        encoding,
        byte_order,
        predicates,
        invalidators,
    })
}

/// Return the payload length of an [`Addressing`] value at a fixed address.
///
/// Returns `None` for an address with a runtime-resolved term: we'd rather
/// opt out of the auto-detection — pointer-backed `<Float>` nodes with no
/// `<Scale>`/`<Offset>` are almost always scaled-integer registers on real
/// cameras, and silently decoding their bytes as IEEE 754 would corrupt the
/// value.
fn addressing_primary_length(addr: &crate::Addressing) -> Option<u32> {
    match addr {
        // A runtime-resolved address says nothing about the payload encoding,
        // so we decline to auto-detect rather than guess.
        crate::Addressing::Sum { terms, len, .. } => terms
            .iter()
            .all(|term| matches!(term, crate::AddressTerm::Fixed(_)))
            .then_some(*len),
    }
}
