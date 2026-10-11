//! Parser for `<Port>` declarations.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::util::{attribute_value_required, read_text_start, skip_element};
use crate::{PortDecl, PortKind, XmlError};

/// Parse a `<Port>` element. `start` must be its start event and `reader`
/// positioned just after it.
///
/// GenICam Standard v2.1.1 §2.8.16 gives a port `<ChunkID>` or `<pChunkID>`,
/// and §2.1.1's node template `<EventID>`; both ids are hexadecimal numbers.
/// A port declaring neither is the device.
///
/// What is refused, so that the registers on such a port refuse to read
/// rather than read the device (ADR-0018):
///
/// - `<pChunkID>`, a chunk id read from another node. No corpus document uses
///   it.
/// - both a `<ChunkID>` and an `<EventID>`: the port would be two things.
///
/// `<SwapEndianess>Yes` and `<CacheChunkData>Yes` are accepted and not
/// honoured, with a warning. Neither appears in the corpus; the standard
/// forbids the first for schema 1.1 and later (§3.1.1), and the second only
/// keeps a chunk readable after its buffer is gone, which this crate's
/// consumer deliberately does not do.
pub fn parse_port(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<PortDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut chunk_id = None;
    let mut event_id = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "ChunkID" => {
                    let text = read_text_start(reader, e)?;
                    chunk_id = Some(parse_hex_id(&name, "ChunkID", &text)?);
                }
                "EventID" => {
                    let text = read_text_start(reader, e)?;
                    event_id = Some(parse_hex_id(&name, "EventID", &text)?);
                }
                "pChunkID" => {
                    return Err(XmlError::Invalid(format!(
                        "port {name} takes its chunk id from <pChunkID>, which is not supported"
                    )));
                }
                tag @ ("SwapEndianess" | "CacheChunkData") => {
                    let tag = tag.to_string();
                    let text = read_text_start(reader, e)?;
                    if text.trim().eq_ignore_ascii_case("yes") {
                        tracing::warn!(port = %name, element = %tag, "not honoured");
                    }
                }
                other => {
                    let other = other.to_string();
                    skip_element(reader, &other)?;
                }
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == "Port" => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!("unterminated Port {name}")));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    let kind = match (chunk_id, event_id) {
        (None, None) => PortKind::Device,
        (Some(chunk_id), None) => PortKind::Chunk { chunk_id },
        (None, Some(event_id)) => PortKind::Event { event_id },
        (Some(_), Some(_)) => {
            return Err(XmlError::Invalid(format!(
                "port {name} declares both a <ChunkID> and an <EventID>"
            )));
        }
    };
    Ok(PortDecl { name, kind })
}

/// A port with no children: `<Port Name="Device"/>`.
pub fn parse_port_empty(start: &BytesStart<'_>) -> Result<PortDecl, XmlError> {
    Ok(PortDecl {
        name: attribute_value_required(start, "Name")?,
        kind: PortKind::Device,
    })
}

/// A `<ChunkID>` or `<EventID>`: hexadecimal, written without a prefix in
/// every corpus document (`0508000E`, `9C43`); a `0x` prefix is tolerated.
fn parse_hex_id(port: &str, element: &str, text: &str) -> Result<u64, XmlError> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    u64::from_str_radix(digits, 16).map_err(|err| {
        XmlError::Invalid(format!(
            "port {port}: <{element}>{trimmed}</{element}> is not hexadecimal: {err}"
        ))
    })
}
