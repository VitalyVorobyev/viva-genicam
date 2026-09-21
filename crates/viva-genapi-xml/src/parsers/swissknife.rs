//! Parser for SwissKnife expression nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{NodeMetaBuilder, TAG_VALUE, handle_predicate_start};
use crate::util::{attribute_value, attribute_value_required, read_text_start, skip_element};
use crate::{FormulaBindings, NodeDecl, PredicateRefs, SkOutput, SwissKnifeDecl, XmlError};

/// Parse a `<SwissKnife>` or `<IntSwissKnife>` element into a
/// [`NodeDecl::SwissKnife`].
///
/// The element name is what decides the arithmetic: `<IntSwissKnife>`
/// evaluates in `i64` and yields an integer, `<SwissKnife>` evaluates in
/// `f64`. Real descriptions carry no `<Output>` element — it appears zero
/// times across the whole vendor corpus while `<IntSwissKnife>` appears over
/// three thousand times — so deriving the type from `<Output>` alone silently
/// made every integer knife a float one. `<Output>` is still honoured as an
/// override for the tools that do emit it.
pub fn parse_swissknife(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut expr: Option<String> = None;
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut output = if start.name().as_ref() == "IntSwissKnife" {
        SkOutput::Integer
    } else {
        SkOutput::Float
    };
    let mut bindings = FormulaBindings::default();
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "Constant" => {
                    let const_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Constant Name=\"{const_name}\">"
                        )));
                    }
                    bindings.constants.push((const_name, trimmed.to_string()));
                }
                // A named `<Expression>` is a sub-formula, not the formula.
                // Only the unnamed spelling aliases `<Formula>`.
                "Expression" | "Formula" => {
                    let sub_name = attribute_value(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression>"
                        )));
                    }
                    match sub_name {
                        Some(sub_name) => {
                            bindings.expressions.push((sub_name, trimmed.to_string()));
                        }
                        None => expr = Some(trimmed.to_string()),
                    }
                }
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <pVariable>"
                        )));
                    }
                    variables.push((var_name, target.to_string()));
                }
                "Output" => {
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
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
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
                "Expression" | "Formula" => {
                    let text = attribute_value_required(e, TAG_VALUE)?;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "SwissKnife node {name} has empty <Expression/>"
                        )));
                    }
                    expr = Some(trimmed.to_string());
                }
                "Output" => {
                    if let Some(value) = attribute_value(e, TAG_VALUE)?
                        && let Some(kind) = SkOutput::parse(&value)
                    {
                        output = kind;
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
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

    let expr = expr
        .ok_or_else(|| XmlError::Invalid(format!("SwissKnife node {name} is missing <Formula>")))?;
    // `<pVariable>` is optional: a constant formula such as `<Formula>0x1234</Formula>`
    // is legal GenICam and appears in the standard's own conformance documents.

    Ok(NodeDecl::SwissKnife(SwissKnifeDecl {
        name,
        meta: meta_builder.build(),
        expr,
        variables,
        output,
        bindings,
        predicates,
    }))
}
