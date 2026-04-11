//! Parsers for Converter, IntConverter, and StringReg nodes.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{NodeMetaBuilder, TAG_P_VALUE};
use crate::builders::AddressingBuilder;
use crate::util::{attribute_value, attribute_value_required, read_text_start, skip_element};
use crate::{
    AccessMode, ConverterDecl, IntConverterDecl, NodeDecl, SkOutput, StringDecl, XmlError,
};

/// Parse a `<Converter>` element into a [`NodeDecl::Converter`].
pub fn parse_converter(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut p_value: Option<String> = None;
    let mut formula_to: Option<String> = None;
    let mut formula_from: Option<String> = None;
    let mut variables_to: Vec<(String, String)> = Vec::new();
    let mut variables_from: Vec<(String, String)> = Vec::new();
    let mut unit: Option<String> = None;
    let mut output = SkOutput::Float;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

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
                b"FormulaTo" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_to = Some(trimmed.to_string());
                    }
                }
                b"FormulaFrom" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_from = Some(trimmed.to_string());
                    }
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "Converter node {name} has empty <pVariable>"
                        )));
                    }
                    // Add to both lists by default; real cameras might use direction hints
                    variables_to.push((var_name.clone(), target.to_string()));
                    variables_from.push((var_name, target.to_string()));
                }
                b"Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                b"Output" | b"Representation" => {
                    let text = read_text_start(reader, e)?;
                    if let Some(kind) = SkOutput::parse(&text) {
                        output = kind;
                    }
                }
                _ => {
                    if !meta_builder.handle_start(reader, e)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    if let Some(value) = attribute_value(e, b"Name")? {
                        p_value = Some(value);
                    }
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    if let Some(target) = attribute_value(e, b"Value")? {
                        if target.is_empty() {
                            return Err(XmlError::Invalid(format!(
                                "Converter node {name} has empty <pVariable/>"
                            )));
                        }
                        variables_to.push((var_name.clone(), target.clone()));
                        variables_from.push((var_name, target));
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Converter node {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let p_value = p_value
        .ok_or_else(|| XmlError::Invalid(format!("Converter node {name} missing <pValue>")))?;

    // FormulaTo converts from raw to user value (reading direction)
    // FormulaFrom converts from user to raw value (writing direction)
    // If missing, use identity: just the pValue variable
    let formula_to = formula_to.unwrap_or_else(|| "FROM".to_string());
    let formula_from = formula_from.unwrap_or_else(|| "TO".to_string());

    // Ensure the pValue is available as a variable named "FROM" in formula_to
    // and "TO" in formula_from (GenICam convention)
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
        unit,
        output,
    }))
}

/// Parse an `<IntConverter>` element into a [`NodeDecl::IntConverter`].
pub fn parse_int_converter(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut p_value: Option<String> = None;
    let mut formula_to: Option<String> = None;
    let mut formula_from: Option<String> = None;
    let mut variables_to: Vec<(String, String)> = Vec::new();
    let mut variables_from: Vec<(String, String)> = Vec::new();
    let mut unit: Option<String> = None;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

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
                b"FormulaTo" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_to = Some(trimmed.to_string());
                    }
                }
                b"FormulaFrom" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        formula_from = Some(trimmed.to_string());
                    }
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if target.is_empty() {
                        return Err(XmlError::Invalid(format!(
                            "IntConverter node {name} has empty <pVariable>"
                        )));
                    }
                    variables_to.push((var_name.clone(), target.to_string()));
                    variables_from.push((var_name, target.to_string()));
                }
                b"Unit" => {
                    let text = read_text_start(reader, e)?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        unit = Some(trimmed.to_string());
                    }
                }
                _ => {
                    if !meta_builder.handle_start(reader, e)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                TAG_P_VALUE => {
                    if let Some(value) = attribute_value(e, b"Name")? {
                        p_value = Some(value);
                    }
                }
                b"pVariable" => {
                    let var_name = attribute_value_required(e, b"Name")?;
                    if let Some(target) = attribute_value(e, b"Value")? {
                        if target.is_empty() {
                            return Err(XmlError::Invalid(format!(
                                "IntConverter node {name} has empty <pVariable/>"
                            )));
                        }
                        variables_to.push((var_name.clone(), target.clone()));
                        variables_from.push((var_name, target));
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated IntConverter node {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let p_value = p_value
        .ok_or_else(|| XmlError::Invalid(format!("IntConverter node {name} missing <pValue>")))?;

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
        unit,
    }))
}

/// Parse a `<StringReg>` or `<String>` element into a [`NodeDecl::String`].
pub fn parse_string(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, b"Name")?;
    let mut addressing = AddressingBuilder::new(&name);
    let mut access = AccessMode::RO;
    let node_name = start.name().as_ref().to_vec();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"Address" => {
                    let text = read_text_start(reader, e)?;
                    let addr = crate::util::parse_u64(&text)?;
                    addressing.attach_selected_address(addr, None);
                }
                b"Length" => {
                    let text = read_text_start(reader, e)?;
                    let value = crate::util::parse_u64(&text)?;
                    let len = u32::try_from(value).map_err(|_| {
                        XmlError::Invalid(format!("length out of range for node {name}"))
                    })?;
                    addressing.apply_length(len);
                }
                b"pAddress" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        addressing.set_p_address_node(target);
                    }
                }
                b"AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                _ => {
                    if !meta_builder.handle_start(reader, e)? {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => {
                if e.name().as_ref() == b"pAddress" {
                    if let Some(value) = attribute_value(e, b"Name")? {
                        let trimmed = value.trim();
                        if !trimmed.is_empty() {
                            addressing.set_p_address_node(trimmed);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_slice() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated String node {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    let addressing = addressing.build();

    Ok(NodeDecl::String(StringDecl {
        name,
        meta: meta_builder.build(),
        addressing,
        access,
    }))
}
