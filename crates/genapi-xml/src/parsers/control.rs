//! Parsers for Command and Category nodes.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::util::{attribute_value, attribute_value_required, parse_u64, read_text_start, skip_element};
use crate::{NodeDecl, XmlError};

/// Parse a `<Command>` element into a [`NodeDecl::Command`].
pub fn parse_command(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut address = None;
    let mut length = None;
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
                    let value = parse_u64(&text)?;
                    length = Some(u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("length out of range for node {name}"))
                    })?);
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Command node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let address = address
        .ok_or_else(|| XmlError::Invalid(format!("Command node {name} is missing <Address>")))?;
    let length = length.unwrap_or(1);

    Ok(NodeDecl::Command {
        name,
        address,
        len: length,
    })
}

/// Parse an empty `<Command />` element.
pub fn parse_command_empty(start: &BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    let address = attribute_value_required(start, b"Address")?;
    let address = parse_u64(&address)?;
    let length = attribute_value(start, b"Length")?;
    let length = match length {
        Some(value) => {
            let raw = parse_u64(&value)?;
            u32::try_from(raw)
                .map_err(|_| XmlError::Invalid("command length out of range".into()))?
        }
        None => 1,
    };
    Ok(NodeDecl::Command {
        name,
        address,
        len: length,
    })
}

/// Parse a `<Category>` element into a [`NodeDecl::Category`].
pub fn parse_category(reader: &mut Reader<&[u8]>, start: BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let node_name = start.name().as_ref().to_vec();
    let mut children = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"pFeature" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        children.push(trimmed.to_string());
                    }
                }
                _ => skip_element(reader, e.name().as_ref())?,
            },
            Ok(Event::Empty(ref e)) if e.name().as_ref() == b"pFeature" => {
                if let Some(value) = attribute_value(e, b"Name")? {
                    if !value.is_empty() {
                        children.push(value);
                    }
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Category node {name}"
                )))
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(NodeDecl::Category { name, children })
}

/// Parse an empty `<Category />` element.
pub fn parse_category_empty(start: &BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(start, b"Name")?;
    Ok(NodeDecl::Category {
        name,
        children: Vec::new(),
    })
}
