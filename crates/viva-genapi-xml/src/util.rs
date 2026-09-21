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
pub fn read_text_start(
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
                return Err(XmlError::Invalid(format!("unterminated <{name}> element")));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
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
    if let Some(ch) = reference
        .resolve_char_ref()
        .map_err(|err| XmlError::Xml(err.to_string()))?
    {
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
pub fn attribute_value(event: &BytesStart<'_>, name: &str) -> Result<Option<String>, XmlError> {
    for attr in event.attributes() {
        let attr = attr.map_err(|err| XmlError::Xml(err.to_string()))?;
        if attr.key.as_ref() == name {
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
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

/// Extract a required attribute value from an XML start element.
pub fn attribute_value_required(event: &BytesStart<'_>, name: &str) -> Result<String, XmlError> {
    attribute_value(event, name)?
        .ok_or_else(|| XmlError::Invalid(format!("missing attribute {name}")))
}

/// Parse an unsigned 64-bit integer from a string (supports hex with `0x` prefix).
pub fn parse_u64(value: &str) -> Result<u64, XmlError> {
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

/// Parse a signed 64-bit integer from a string (supports hex with `0x` prefix).
pub fn parse_i64(value: &str) -> Result<i64, XmlError> {
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

/// Parse a 64-bit floating point number from a string.
pub fn parse_f64(value: &str) -> Result<f64, XmlError> {
    value
        .trim()
        .parse()
        .map_err(|err| XmlError::Invalid(format!("invalid float: {err}")))
}

/// Skip over an XML element and all of its children.
pub fn skip_element(reader: &mut Reader<&[u8]>, _name: &str) -> Result<(), XmlError> {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    while depth > 0 {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
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

/// Parse a scale factor as a rational number (numerator, denominator).
pub fn parse_scale(text: &str) -> Result<(i64, i64), XmlError> {
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
