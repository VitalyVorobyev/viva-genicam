//! Parser for SwissKnife expression nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{NodeMetaBuilder, TAG_VALUE, handle_predicate_start};
use crate::util::{attribute_value, attribute_value_required, read_text_start, skip_element};
use crate::{NodeDecl, PredicateRefs, SkOutput, SwissKnifeDecl, XmlError};

/// Parse a `<SwissKnife>` element into a [`NodeDecl::SwissKnife`].
pub fn parse_swissknife(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut expr: Option<String> = None;
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut output = SkOutput::Float;
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Expression" | b"Formula" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression>"
                        )));
                    }
                    expr = Some(trimmed.to_string());
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <pVariable>"
                        )));
                    }
                    variables.push((var_name, target.to_string()));
                }
                b"Output" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(kind) = SkOutput::parse(&text) {
                        output = kind;
                    }
                }
                _ => {
                    if handle_predicate_start(reader, e, &mut predicates)? {
                        // handled
                    } else if !meta_builder.handle_start(reader, e)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    if let Some(target) = attribute_value(e, TAG_VALUE)? {
                        if target.is_empty() {
                            return Err(XmlError::Invalid(format!(
                                "SwissKnife node {name} has empty <pVariable/>"
                            )));
                        }
                        variables.push((var_name, target));
                    } else {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} missing variable target"
                        )));
                    }
                }
                b"Expression" | b"Formula" => {
                    let text = attribute_value_required(e, TAG_VALUE)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression/>"
                        )));
                    }
                    expr = Some(trimmed.to_string());
                }
                b"Output" => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)?
                        && let Some(kind) = SkOutput::parse(&value)
                    {
                        output = kind;
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated SwissKnife node {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let expr = expr.ok_or_else(|| {
        XmlError::Invalid(format!("SwissKnife node {name} is missing <Expression>"))
    })?;
    if variables.is_empty() {
        return Err(XmlError::Invalid(format!(
            "SwissKnife node {name} must declare at least one <pVariable>"
        )));
    }

    Ok(NodeDecl::SwissKnife(SwissKnifeDecl {
        name,
        meta: meta_builder.build(),
        expr,
        variables,
        output,
        predicates,
    }))
}
