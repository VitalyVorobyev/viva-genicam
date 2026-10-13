//! Parsers for Command and Category nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{NodeMetaBuilder, handle_predicate_start};
use crate::util::{
    attribute_value, attribute_value_required, parse_i64, parse_u64, read_text_start,
};
use crate::{CommandValueSrc, NodeDecl, NodeMeta, PredicateRefs, XmlError};

/// Parse a `<Command>` element into a [`NodeDecl::Command`].
pub(crate) fn parse_command(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut address = None;
    let mut length = None;
    let mut pvalue = None;
    let mut command_value = None;
    let mut p_command_value = None;
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "Address" => {
                    let text = read_text_start(reader, e)?;
                    address = Some(parse_u64(&text)?);
                }
                "Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = parse_u64(&text)?;
                    length = Some(u32::try_from(value).map_err(|_| {
                        XmlError::invalid(&name, "declares a <Length> out of range")
                    })?);
                }
                "pValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                "CommandValue" => {
                    let text = read_text_start(reader, e)?;
                    command_value = Some(parse_i64(&text)?);
                }
                "pCommandValue" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        p_command_value = Some(target.to_string());
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
            Ok(Event::Empty(ref e)) => meta_builder.ignore_empty(e),
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    // Commands may lack both <Address> and <pValue> in real-world XML
    // (e.g. SFNC standard features without register backing).
    let length = length.unwrap_or(4);

    // The schema makes the two a choice; a document declaring both is
    // ambiguous, and the dynamic one is the one the device keeps current.
    let command_value = match (p_command_value, command_value) {
        (Some(node), _) => CommandValueSrc::FromNode(node),
        (None, Some(value)) => CommandValueSrc::Literal(value),
        (None, None) => return Err(missing_command_value(&name)),
    };

    Ok(NodeDecl::Command {
        name,
        meta: meta_builder.build(),
        address,
        len: length,
        pvalue,
        command_value,
        predicates,
    })
}

/// Why a `<Command>` with no value is skipped.
///
/// GenICam Standard v2.1.1 §2.8.8 gives a command its value through
/// `<CommandValue>` or `<pCommandValue>`, and §3.2's table of defaults has no
/// entry for either, so there is no value to fall back on. All 490 commands in
/// the 38-document vendor corpus declare one. Writing an invented `1` instead
/// is how a user-set command saves into the wrong set, so the node is refused
/// and the reason recorded.
pub(crate) fn missing_command_value(name: &str) -> XmlError {
    XmlError::missing(name, "<CommandValue> or <pCommandValue>")
}

/// Parse a `<Category>` element into a [`NodeDecl::Category`].
pub(crate) fn parse_category(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let node_name = start.name().as_ref().to_string();
    let mut children = Vec::new();
    let mut predicates = PredicateRefs::default();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "pFeature" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        children.push(trimmed.to_string());
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
            Ok(Event::Empty(ref e)) if e.name().as_ref() == "pFeature" => {
                if let Some(value) = attribute_value(e, "Name")?
                    && !value.is_empty()
                {
                    children.push(value);
                }
            }
            Ok(Event::Empty(ref e)) => meta_builder.ignore_empty(e),
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    Ok(NodeDecl::Category {
        name,
        meta: meta_builder.build(),
        children,
        predicates,
    })
}

/// Parse an empty `<Category />` element.
pub(crate) fn parse_category_empty(start: &BytesStart<'_>) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(start, "Name")?;
    Ok(NodeDecl::Category {
        name,
        meta: NodeMeta::default(),
        children: Vec::new(),
        predicates: PredicateRefs::default(),
    })
}
