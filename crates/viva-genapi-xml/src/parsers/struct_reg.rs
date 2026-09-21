//! Parser for `<StructReg>` elements that contain `<StructEntry>` children.
//!
//! Each `StructEntry` becomes a separate `NodeDecl::Integer` sharing the same
//! register address but with unique bitfield metadata. The shared parts —
//! address terms, length, access mode, byte order, sign — are declared once on
//! the `<StructReg>` and inherited; an entry may override the access mode and
//! sign for its own bits.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{TAG_P_ADDRESS, TAG_P_INDEX, index_offset};
use crate::util::{
    attribute_value, attribute_value_required, parse_u64, read_text_start, skip_element,
};
use crate::{
    AccessMode, AddressTerm, Addressing, BitField, ByteOrder, NodeDecl, NodeMeta, PredicateRefs,
    Sign, XmlError,
};

/// Parse a `<StructReg>` element, producing one `NodeDecl::Integer` per `<StructEntry>`.
pub fn parse_struct_reg(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<Vec<NodeDecl>, XmlError> {
    // Address terms sum, exactly as they do for a plain register. Point Grey
    // and Hikrobot both write `<Address>` alongside `<pAddress>` here, so
    // keeping only one of them puts every inquiry bit in the document at the
    // wrong address — 860 of them on the camera in issue #35.
    let mut terms: Vec<AddressTerm> = Vec::new();
    let mut length: u32 = 4;
    let mut access = AccessMode::RW;
    let mut byte_order = ByteOrder::Little;
    let mut sign = Sign::default();
    let mut entries = Vec::new();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "Address" => {
                    let text = read_text_start(reader, e)?;
                    terms.push(AddressTerm::Fixed(parse_u64(&text)?));
                }
                TAG_P_ADDRESS => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        terms.push(AddressTerm::Node(target.to_string()));
                    }
                }
                TAG_P_INDEX => {
                    let offset = index_offset(e)?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        terms.push(AddressTerm::Index {
                            node: target.to_string(),
                            offset,
                        });
                    }
                }
                "Length" => {
                    let text = read_text_start(reader, e)?;
                    length = parse_u64(&text)? as u32;
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                "Sign" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(parsed) = Sign::parse(&text) {
                        sign = parsed;
                    }
                }
                "Endianness" | "Endianess" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        byte_order = order;
                    }
                }
                "StructEntry" => {
                    let entry = parse_struct_entry(reader, e.clone(), byte_order)?;
                    entries.push(entry);
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_ADDRESS => {
                    if let Some(value) = attribute_value(e, "Name")? {
                        let trimmed = value.trim();
                        if !trimmed.is_empty() {
                            terms.push(AddressTerm::Node(trimmed.to_string()));
                        }
                    }
                }
                TAG_P_INDEX => {
                    if let Some(value) = attribute_value(e, "Name")? {
                        let trimmed = value.trim();
                        if !trimmed.is_empty() {
                            terms.push(AddressTerm::Index {
                                node: trimmed.to_string(),
                                offset: index_offset(e)?,
                            });
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid("unterminated StructReg".into()));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if terms.is_empty() {
        // Defaulting to address 0 here would produce a nodemap full of
        // registers that read the wrong memory and never say so.
        return Err(XmlError::Invalid(
            "StructReg declares no <Address>, <pAddress> or <pIndex>".into(),
        ));
    }
    let addressing = Addressing::Sum { terms, len: length };

    // Convert each StructEntry into a NodeDecl::Integer with shared addressing.
    let nodes = entries
        .into_iter()
        .map(|entry| {
            let entry_sign = entry.sign.unwrap_or(sign);
            let (min, max) = bitfield_range(entry.bitfield.bit_length, entry_sign);
            NodeDecl::Integer {
                name: entry.name,
                meta: NodeMeta::default(),
                addressing: Some(addressing.clone()),
                len: length,
                access: entry.access.unwrap_or(access),
                min,
                max,
                inc: None,
                unit: None,
                bitfield: Some(entry.bitfield),
                sign: entry_sign,
                selectors: Vec::new(),
                selected_if: Vec::new(),
                pvalue: None,
                p_max: None,
                p_min: None,
                value: None,
                predicates: PredicateRefs::default(),
            }
        })
        .collect();

    Ok(nodes)
}

/// Value range a bitfield of `bits` can hold.
///
/// Declaring the true range rather than the full `i64` span matters: a
/// one-bit entry that claims a minimum of `i64::MIN` looks like a signed
/// field, and reading it then sign-extends a set bit to `-1`.
fn bitfield_range(bits: u16, sign: Sign) -> (i64, i64) {
    let bits = bits.clamp(1, 64) as u32;
    if sign.is_signed() {
        if bits == 1 {
            // A single signed bit can only be 0 or -1.
            return (-1, 0);
        }
        let magnitude = 1i64 << (bits - 1);
        (-magnitude, magnitude - 1)
    } else if bits >= 63 {
        (0, i64::MAX)
    } else {
        (0, (1i64 << bits) - 1)
    }
}

struct StructEntryData {
    name: String,
    bitfield: BitField,
    /// `<AccessMode>` declared on the entry, overriding the `<StructReg>` one.
    access: Option<AccessMode>,
    /// `<Sign>` declared on the entry, overriding the `<StructReg>` one.
    sign: Option<Sign>,
}

fn parse_struct_entry(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
    default_byte_order: ByteOrder,
) -> Result<StructEntryData, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut lsb: Option<u16> = None;
    let mut msb: Option<u16> = None;
    let mut bit: Option<u16> = None;
    let mut access: Option<AccessMode> = None;
    let mut sign: Option<Sign> = None;
    let mut byte_order = default_byte_order;
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "LSB" | "Lsb" => {
                    let text = read_text_start(reader, e)?;
                    lsb = Some(parse_u64(&text)? as u16);
                }
                "MSB" | "Msb" => {
                    let text = read_text_start(reader, e)?;
                    msb = Some(parse_u64(&text)? as u16);
                }
                "Bit" => {
                    let text = read_text_start(reader, e)?;
                    bit = Some(parse_u64(&text)? as u16);
                }
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = Some(AccessMode::parse(&text)?);
                }
                "Sign" => {
                    let text = read_text_start(reader, e)?;
                    sign = Sign::parse(&text);
                }
                "Endianness" | "Endianess" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        byte_order = order;
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated StructEntry {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let bitfield = if let Some(single) = bit {
        BitField {
            bit_offset: single,
            bit_length: 1,
            byte_order,
        }
    } else if let (Some(l), Some(m)) = (lsb, msb) {
        let (low, high) = if l <= m { (l, m) } else { (m, l) };
        BitField {
            bit_offset: low,
            bit_length: high - low + 1,
            byte_order,
        }
    } else {
        return Err(XmlError::Invalid(format!(
            "StructEntry {name} needs LSB+MSB or Bit"
        )));
    };

    Ok(StructEntryData {
        name,
        bitfield,
        access,
        sign,
    })
}
