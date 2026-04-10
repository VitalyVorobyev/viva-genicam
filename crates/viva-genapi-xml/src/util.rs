//! Low-level parsing utilities for text extraction and number conversion.

use quick_xml::events::BytesStart;
use quick_xml::name::QName;
use quick_xml::Reader;

use crate::XmlError;

/// Read text content from a start element until its closing tag.
pub fn read_text_start(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
) -> Result<String, XmlError> {
    let end_buf = start.name().as_ref().to_vec();
    let text = reader
        .read_text(QName(&end_buf))
        .map_err(|err| XmlError::Xml(err.to_string()))?;
    // Unescape XML entities (&amp; → &, &lt; → <, etc.).
    quick_xml::escape::unescape(&text)
        .map(|cow| cow.into_owned())
        .map_err(|err| XmlError::Xml(format!("unescape error: {err}")))
}

/// Extract an optional attribute value from an XML start element.
pub fn attribute_value(event: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>, XmlError> {
    for attr in event.attributes() {
        let attr = attr.map_err(|err| XmlError::Xml(err.to_string()))?;
        if attr.key.as_ref() == name {
            let value = attr
                .unescape_value()
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
pub fn attribute_value_required(event: &BytesStart<'_>, name: &[u8]) -> Result<String, XmlError> {
    attribute_value(event, name)?.ok_or_else(|| {
        XmlError::Invalid(format!(
            "missing attribute {}",
            String::from_utf8_lossy(name)
        ))
    })
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
pub fn skip_element(reader: &mut Reader<&[u8]>, _name: &[u8]) -> Result<(), XmlError> {
    use quick_xml::events::Event;
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
