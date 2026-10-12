//! Low-level parsing utilities for text extraction and number conversion.

use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::XmlError;

/// Read the text content of a start element up to its closing tag.
///
/// The result is the element's string value: character data plus CDATA sections
/// plus resolved entity references, with comments and processing instructions
/// dropped.
///
/// Deliberately *not* implemented via `Reader::read_text` + `escape::unescape`:
/// `read_text` hands back the raw span including markup, and blanket-unescaping
/// that span corrupts (or rejects) content where a bare `&` is legal XML — CDATA
/// sections and comments. Vendor XML does use those, and a single tooltip must
/// never block opening a camera (issue #45).
pub(crate) fn read_text_start(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
) -> Result<String, XmlError> {
    // Documents are read with `trim_text(true)` for structural parsing, but the
    // reader splits character data at entity references, so trimming each
    // fragment would eat the spaces around `&amp;`. Callers trim the result.
    let trim_start = reader.config().trim_text_start;
    let trim_end = reader.config().trim_text_end;
    reader.config_mut().trim_text(false);

    let result = read_text_events(reader, start.name().as_ref());

    reader.config_mut().trim_text_start = trim_start;
    reader.config_mut().trim_text_end = trim_end;
    result
}

/// Event loop backing [`read_text_start`], split out so the caller can restore
/// the reader's trim configuration on both the success and the error path.
fn read_text_events(reader: &mut Reader<&[u8]>, name: &str) -> Result<String, XmlError> {
    let mut text = String::new();
    let mut depth = 1usize;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Text(chunk)) => text.push_str(&chunk),
            Ok(Event::CData(chunk)) => {
                // CDATA content is literal: no entity resolution.
                text.push_str(&chunk);
            }
            Ok(Event::GeneralRef(reference)) => push_reference(&mut text, &reference)?,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            // Comments, processing instructions and declarations carry no text.
            _ => {}
        }
        buf.clear();
    }

    Ok(text)
}

/// Append a resolved `&…;` reference to `text`.
///
/// Character references and the five entities predefined by XML are resolved.
/// GenICam documents declare no DTD, so anything else has no definition to
/// resolve against and is kept verbatim rather than failing the document.
fn push_reference(text: &mut String, reference: &BytesRef<'_>) -> Result<(), XmlError> {
    if let Some(ch) = reference.resolve_char_ref().map_err(XmlError::malformed)? {
        text.push(ch);
        return Ok(());
    }
    match &**reference {
        "lt" => text.push('<'),
        "gt" => text.push('>'),
        "amp" => text.push('&'),
        "apos" => text.push('\''),
        "quot" => text.push('"'),
        other => {
            text.push('&');
            text.push_str(other);
            text.push(';');
        }
    }
    Ok(())
}

/// Extract an optional attribute value from an XML start element.
pub(crate) fn attribute_value(
    event: &BytesStart<'_>,
    name: &str,
) -> Result<Option<String>, XmlError> {
    for attr in event.attributes() {
        let attr = attr.map_err(XmlError::malformed)?;
        if attr.key.as_ref() == name {
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(XmlError::malformed)?;
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                return Ok(None);
            }
            return Ok(Some(trimmed));
        }
    }
    Ok(None)
}

/// Extract a required attribute value from an XML start element.
pub(crate) fn attribute_value_required(
    event: &BytesStart<'_>,
    name: &'static str,
) -> Result<String, XmlError> {
    attribute_value(event, name)?.ok_or_else(|| {
        let tag = event.name().as_ref().to_string();
        XmlError::missing(tag, missing_attribute(name))
    })
}

/// How [`XmlError::Missing`] names a missing attribute.
fn missing_attribute(name: &'static str) -> &'static str {
    match name {
        "Name" => "attribute Name",
        _ => "a required attribute",
    }
}

/// Parse an unsigned 64-bit integer from a string (supports hex with `0x` prefix).
pub(crate) fn parse_u64(value: &str) -> Result<u64, XmlError> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let hex = hex.replace('_', "");
        u64::from_str_radix(&hex, 16)
            .map_err(|err| XmlError::number(trimmed, "a hexadecimal integer", err))
    } else {
        let dec = trimmed.replace('_', "");
        dec.parse()
            .map_err(|err| XmlError::number(trimmed, "an unsigned integer", err))
    }
}

/// Parse an `<Address>`: signed decimal, or hex whose bit pattern is kept.
///
/// GenICam Standard v2.1.1 §2.8.16 allows a negative address on a chunk port
/// (`<Address>-4</Address>`, Baumer HXG20). A hex address is read as the
/// unsigned 64-bit pattern it spells and reinterpreted, so `0xFFFFFFFC`-style
/// device addresses keep their value and a full-width `0xFF…FC` is `-4`.
pub(crate) fn parse_address(value: &str) -> Result<i64, XmlError> {
    let trimmed = value.trim();
    if trimmed.starts_with("0x") {
        parse_u64(trimmed).map(|v| v as i64)
    } else {
        let dec = trimmed.replace('_', "");
        dec.parse()
            .map_err(|err| XmlError::number(trimmed, "an address", err))
    }
}

/// Parse a signed 64-bit integer from a string (supports hex with `0x` prefix).
pub(crate) fn parse_i64(value: &str) -> Result<i64, XmlError> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let hex = hex.replace('_', "");
        i64::from_str_radix(&hex, 16)
            .map_err(|err| XmlError::number(trimmed, "a hexadecimal integer", err))
    } else {
        let dec = trimmed.replace('_', "");
        dec.parse()
            .map_err(|err| XmlError::number(trimmed, "an integer", err))
    }
}

/// Parse a 64-bit floating point number from a string.
pub(crate) fn parse_f64(value: &str) -> Result<f64, XmlError> {
    let trimmed = value.trim();
    trimmed
        .parse()
        .map_err(|err| XmlError::number(trimmed, "a number", err))
}

/// Skip over an XML element and all of its children.
pub(crate) fn skip_element(reader: &mut Reader<&[u8]>, name: &str) -> Result<(), XmlError> {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    while depth > 0 {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
            }
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Parse a scale factor as a rational number (numerator, denominator).
pub(crate) fn parse_scale(text: &str) -> Result<(i64, i64), XmlError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(XmlError::invalid("<Scale>", "is empty"));
    }
    if let Some((num, den)) = trimmed.split_once('/') {
        let num = parse_i64(num)?;
        let den = parse_i64(den)?;
        if den == 0 {
            return Err(XmlError::invalid("<Scale>", "has a zero denominator"));
        }
        Ok((num, den))
    } else {
        let value = parse_f64(trimmed)?;
        if value == 0.0 {
            return Err(XmlError::invalid("<Scale>", "is zero"));
        }
        // Approximate decimal scale as a rational using denominator 1_000_000.
        let den = 1_000_000i64;
        let num = (value * den as f64).round() as i64;
        Ok((num, den))
    }
}
