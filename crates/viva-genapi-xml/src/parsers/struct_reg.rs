//! Parser for `<StructReg>` elements that contain `<StructEntry>` children.
//!
//! Each `StructEntry` becomes a separate `NodeDecl::Integer` sharing the same
//! register address but with unique bitfield metadata.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::util::{attribute_value_required, parse_u64, read_text_start, skip_element};
use crate::{AccessMode, Addressing, BitField, ByteOrder, NodeDecl, XmlError};

/// Parse a `<StructReg>` element, producing one `NodeDecl::Integer` per `<StructEntry>`.
pub fn parse_struct_reg(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<Vec<NodeDecl>, XmlError> {
    let mut address: Option<u64> = None;
    let mut length: u32 = 4;
    let mut access = AccessMode::RW;
    let mut byte_order = ByteOrder::Little;
    let mut entries = Vec::new();
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
                    length = parse_u64(&text)? as u32;
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                b"Endianness" | b"Endianess" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(order) = ByteOrder::parse(&text) {
                        byte_order = order;
                    }
                }
                b"StructEntry" => {
                    let entry = parse_struct_entry(reader, e.clone(), byte_order)?;
                    entries.push(entry);
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid("unterminated StructReg".into()));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let addr = address.unwrap_or(0);
    let addressing = Addressing::Fixed {
        address: addr,
        len: length,
    };

    // Convert each StructEntry into a NodeDecl::Integer with shared addressing.
    let nodes = entries
        .into_iter()
        .map(|entry| NodeDecl::Integer {
            name: entry.name,
            addressing: Some(addressing.clone()),
            len: length,
            access,
            min: i64::MIN,
            max: i64::MAX,
            inc: None,
            unit: None,
            bitfield: Some(entry.bitfield),
            selectors: Vec::new(),
            selected_if: Vec::new(),
            pvalue: None,
            p_max: None,
            p_min: None,
            value: None,
        })
        .collect();

    Ok(nodes)
}

struct StructEntryData {
    name: String,
    bitfield: BitField,
}

fn parse_struct_entry(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
    default_byte_order: ByteOrder,
) -> Result<StructEntryData, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut lsb: Option<u16> = None;
    let mut msb: Option<u16> = None;
    let mut bit: Option<u16> = None;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"LSB" => {
                    let text = read_text_start(reader, e)?;
                    lsb = Some(parse_u64(&text)? as u16);
                }
                b"MSB" => {
                    let text = read_text_start(reader, e)?;
                    msb = Some(parse_u64(&text)? as u16);
                }
                b"Bit" => {
                    let text = read_text_start(reader, e)?;
                    bit = Some(parse_u64(&text)? as u16);
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
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
            byte_order: default_byte_order,
        }
    } else if let (Some(l), Some(m)) = (lsb, msb) {
        let (low, high) = if l <= m { (l, m) } else { (m, l) };
        BitField {
            bit_offset: low,
            bit_length: high - low + 1,
            byte_order: default_byte_order,
        }
    } else {
        return Err(XmlError::Invalid(format!(
            "StructEntry {name} needs LSB+MSB or Bit"
        )));
    };

    Ok(StructEntryData { name, bitfield })
}
