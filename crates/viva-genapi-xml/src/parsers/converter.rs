//! Parsers for Converter, IntConverter, StringReg and String nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{
    NodeMetaBuilder, TAG_P_VALUE, TAG_VALUE, handle_addressing_empty, handle_addressing_start,
    handle_invalidator_start, handle_predicate_start,
};
use crate::builders::AddressingBuilder;
use crate::util::{attribute_value, attribute_value_required, read_text_start};
use crate::{
    AccessMode, ConverterDecl, FormulaBindings, IntConverterDecl, NodeDecl, PredicateRefs,
    SkOutput, Slope, StringDecl, XmlError,
};

/// Read a `<Slope>` element.
///
/// The schema allows four values. One we do not recognise falls back to
/// `Automatic`, the standard's default, which finds the direction by probing
/// the formula rather than assuming one.
fn read_slope(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    node: &str,
) -> Result<Slope, XmlError> {
    let text = read_text_start(reader, event)?;
    Ok(Slope::parse(&text).unwrap_or_else(|| {
        tracing::warn!(node, slope = %text.trim(), "unknown <Slope>; probing instead");
        Slope::Automatic
    }))
}

/// Parse a `<Converter>` element into a [`NodeDecl::Converter`].
pub(crate) fn parse_converter(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut p_value: Option<String> = None;
    let mut formula_to: Option<String> = None;
    let mut formula_from: Option<String> = None;
    let mut variables_to: Vec<(String, String)> = Vec::new();
    let mut variables_from: Vec<(String, String)> = Vec::new();
    let mut unit: Option<String> = None;
    let mut output = SkOutput::Float;
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut bindings = FormulaBindings::default();
    let mut meta_builder = NodeMetaBuilder::default();
    let mut slope = Slope::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        p_value = Some(trimmed.to_string());
                    }
                }
                "FormulaTo" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_to = Some(trimmed.to_string());
                    }
                }
                "FormulaFrom" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_from = Some(trimmed.to_string());
                    }
                }
                // Named literals and sub-formulas, resolved by substitution
                // when the runtime builds the node.
                "Constant" => {
                    let const_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        bindings.constants.push((const_name, trimmed.to_string()));
                    }
                }
                "Expression" => {
                    let sub_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        bindings.expressions.push((sub_name, trimmed.to_string()));
                    }
                }
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::invalid(&name, "has an empty <pVariable>"));
                    }
                    // Add to both lists by default; real cameras might use direction hints
                    variables_to.push((var_name.clone(), target.to_string()));
                    variables_from.push((var_name, target.to_string()));
                }
                "Slope" => {
                    slope = read_slope(reader, e, &name)?;
                }
                "Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                "Output" | "Representation" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(kind) = SkOutput::parse(&text) {
                        output = kind;
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
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    if let Some(value) = attribute_value(e, "Name")? {
                        p_value = Some(value);
                    }
                }
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
                    if let Some(target) = attribute_value(e, "Value")? {
                        if target.is_empty() {
                            return Err(XmlError::invalid(&name, "has an empty <pVariable>"));
                        }
                        variables_to.push((var_name.clone(), target.clone()));
                        variables_from.push((var_name, target));
                    }
                }
                _ => meta_builder.ignore_empty(e),
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    let p_value = p_value.ok_or_else(|| XmlError::missing(&name, "<pValue>"))?;

    // `<FormulaFrom>` converts the raw value to the feature value — the read
    // direction — and binds `TO` to `<pValue>`. `<FormulaTo>` goes the other
    // way and binds `FROM` to the value being written. Missing formulas
    // default to the identity.
    let formula_to = formula_to.unwrap_or_else(|| "FROM".to_string());
    let formula_from = formula_from.unwrap_or_else(|| "TO".to_string());

    // `TO` names the raw value in the read direction, so bind it to
    // `<pValue>`. `FROM` names the value being written, which only exists at
    // write time — the write path rebinds it; the fallback here just keeps the
    // formula resolvable when it is inspected offline.
    if !variables_to.iter().any(|(n, _)| n == "FROM") {
        variables_to.push(("FROM".to_string(), p_value.clone()));
    }
    if !variables_from.iter().any(|(n, _)| n == "TO") {
        variables_from.push(("TO".to_string(), p_value.clone()));
    }

    Ok(NodeDecl::Converter(ConverterDecl {
        name,
        meta: meta_builder.build(),
        p_value,
        formula_to,
        formula_from,
        variables_to,
        variables_from,
        bindings,
        unit,
        output,
        slope,
        predicates,
    }))
}

/// Parse an `<IntConverter>` element into a [`NodeDecl::IntConverter`].
pub(crate) fn parse_int_converter(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut p_value: Option<String> = None;
    let mut formula_to: Option<String> = None;
    let mut formula_from: Option<String> = None;
    let mut variables_to: Vec<(String, String)> = Vec::new();
    let mut variables_from: Vec<(String, String)> = Vec::new();
    let mut unit: Option<String> = None;
    let mut predicates = PredicateRefs::default();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut bindings = FormulaBindings::default();
    let mut meta_builder = NodeMetaBuilder::default();
    let mut slope = Slope::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        p_value = Some(trimmed.to_string());
                    }
                }
                "FormulaTo" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_to = Some(trimmed.to_string());
                    }
                }
                "FormulaFrom" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_from = Some(trimmed.to_string());
                    }
                }
                // Named literals and sub-formulas, resolved by substitution
                // when the runtime builds the node.
                "Constant" => {
                    let const_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        bindings.constants.push((const_name, trimmed.to_string()));
                    }
                }
                "Expression" => {
                    let sub_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        bindings.expressions.push((sub_name, trimmed.to_string()));
                    }
                }
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::invalid(&name, "has an empty <pVariable>"));
                    }
                    variables_to.push((var_name.clone(), target.to_string()));
                    variables_from.push((var_name, target.to_string()));
                }
                "Slope" => {
                    slope = read_slope(reader, e, &name)?;
                }
                "Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
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
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    if let Some(value) = attribute_value(e, "Name")? {
                        p_value = Some(value);
                    }
                }
                "pVariable" => {
                    let var_name = attribute_value_required(e, "Name")?;
                    if let Some(target) = attribute_value(e, "Value")? {
                        if target.is_empty() {
                            return Err(XmlError::invalid(&name, "has an empty <pVariable>"));
                        }
                        variables_to.push((var_name.clone(), target.clone()));
                        variables_from.push((var_name, target));
                    }
                }
                _ => meta_builder.ignore_empty(e),
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    let p_value = p_value.ok_or_else(|| XmlError::missing(&name, "<pValue>"))?;

    let formula_to = formula_to.unwrap_or_else(|| "FROM".to_string());
    let formula_from = formula_from.unwrap_or_else(|| "TO".to_string());

    if !variables_to.iter().any(|(n, _)| n == "FROM") {
        variables_to.push(("FROM".to_string(), p_value.clone()));
    }
    if !variables_from.iter().any(|(n, _)| n == "TO") {
        variables_from.push(("TO".to_string(), p_value.clone()));
    }

    Ok(NodeDecl::IntConverter(IntConverterDecl {
        name,
        meta: meta_builder.build(),
        p_value,
        formula_to,
        formula_from,
        variables_to,
        variables_from,
        bindings,
        unit,
        slope,
        predicates,
    }))
}

/// Parse a `<StringReg>` or `<String>` element into a [`NodeDecl::String`].
///
/// The two are different nodes (GenICam Standard v2.1.1 §2.8.11 and
/// §2.8.12). A `<StringReg>` is a register: an address, a `<Length>` or
/// `<pLength>`, an `<AccessMode>` that defaults to read-only. A `<String>`
/// has no register: it holds a `<Value>`, a floating node, or reads and
/// writes the node its `<pValue>` names. Reading a `<String>` as a register
/// is how the mvBlueFOX3-2004G's `DeviceFirmwareVersion` read empty.
pub(crate) fn parse_string(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let node_name = start.name().as_ref().to_string();
    let is_register = node_name == "StringReg";
    let mut addressing = AddressingBuilder::default();
    let mut access = if is_register {
        AccessMode::RO
    } else {
        AccessMode::RW
    };
    let mut pvalue: Option<String> = None;
    let mut value: Option<String> = None;
    let mut predicates = PredicateRefs::default();
    let mut invalidators = Vec::new();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                TAG_P_VALUE if !is_register => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        pvalue = Some(target.to_string());
                    }
                }
                // The text as written: a string's spaces are its own.
                TAG_VALUE if !is_register => {
                    value = Some(read_text_start(reader, e)?);
                }
                _ => {
                    // `<Address>`, `<pAddress>`, `<pIndex>`, `<Length>`,
                    // `<pLength>` and `<pPort>` mean what they mean on any
                    // register.
                    if handle_addressing_start(reader, e, &name, &mut addressing)?
                        || handle_predicate_start(reader, e, &mut predicates)?
                        || handle_invalidator_start(reader, e, &mut invalidators)?
                    {
                        // handled
                    } else {
                        meta_builder.handle_or_ignore(reader, e)?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                // `<Value/>`: an empty start value.
                TAG_VALUE if !is_register => value = Some(String::new()),
                _ => {
                    if !handle_addressing_empty(e, &mut addressing)? {
                        meta_builder.ignore_empty(e);
                    }
                }
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    // A `<String>` that declares a register anyway keeps it: the address is
    // what the document says, and nothing else would give the node a value.
    let addressing = if is_register || addressing.has_terms() {
        Some(addressing.build())
    } else {
        None
    };
    if addressing.is_none() && pvalue.is_none() && value.is_none() {
        return Err(XmlError::missing(&name, "<Value> or <pValue>"));
    }

    Ok(NodeDecl::String(StringDecl {
        name,
        meta: meta_builder.build(),
        addressing,
        pvalue,
        value,
        access,
        predicates,
        invalidators,
    }))
}
