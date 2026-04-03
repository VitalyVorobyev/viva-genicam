//! NodeMap implementation for runtime feature access.

use std::cell::Cell;
use std::collections::{hash_map::Entry as HashMapEntry, HashMap, HashSet};

use genapi_xml::{AccessMode, Addressing, EnumEntryDecl, EnumValueSrc, NodeDecl, XmlModel};
use tracing::{debug, trace, warn};

use crate::bitops::{extract, insert};
use crate::conversions::{
    apply_scale, bytes_to_i64, encode_bitfield_value, encode_float, get_raw_or_read, i64_to_bytes,
    interpret_bitfield_value, map_bitops_error, round_to_i64,
};
use crate::nodes::{
    BooleanNode, CategoryNode, CommandNode, ConverterNode, EnumMapping, EnumNode, FloatNode,
    IntConverterNode, IntegerNode, Node, SkNode, StringNode,
};
use crate::swissknife::{
    collect_identifiers, evaluate as eval_ast, parse_expression, EvalError as SkEvalError,
};
use crate::{GenApiError, RegisterIo, SkOutput};

/// Runtime nodemap built from an [`XmlModel`] capable of reading and writing
/// feature values via a [`RegisterIo`] transport.
#[derive(Debug)]
pub struct NodeMap {
    version: String,
    nodes: HashMap<String, Node>,
    dependents: HashMap<String, Vec<String>>,
    generation: Cell<u64>,
}

fn register_addressing_dependency(
    dependents: &mut HashMap<String, Vec<String>>,
    node_name: &str,
    addressing: &Addressing,
) {
    match addressing {
        Addressing::Fixed { .. } => {}
        Addressing::BySelector { selector, .. } => {
            dependents
                .entry(selector.clone())
                .or_default()
                .push(node_name.to_string());
        }
        Addressing::Indirect { p_address_node, .. } => {
            dependents
                .entry(p_address_node.clone())
                .or_default()
                .push(node_name.to_string());
        }
    }
}

fn ensure_readable(access: &AccessMode, name: &str) -> Result<(), GenApiError> {
    if matches!(access, AccessMode::WO) {
        return Err(GenApiError::Access(name.to_string()));
    }
    Ok(())
}

fn ensure_writable(access: &AccessMode, name: &str) -> Result<(), GenApiError> {
    if matches!(access, AccessMode::RO) {
        return Err(GenApiError::Access(name.to_string()));
    }
    Ok(())
}

impl NodeMap {
    /// Return the schema version string associated with the XML description.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Fetch a node by name for inspection.
    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.get(name)
    }

    /// Construct a [`NodeMap`] from an [`XmlModel`], validating SwissKnife expressions.
    pub fn try_from_xml(model: XmlModel) -> Result<Self, GenApiError> {
        let mut nodes = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for decl in model.nodes {
            match decl {
                NodeDecl::Integer {
                    name,
                    addressing,
                    len,
                    access,
                    min,
                    max,
                    inc,
                    unit,
                    bitfield,
                    selectors,
                    selected_if,
                    pvalue,
                    p_max,
                    p_min,
                    value,
                } => {
                    if let Some(ref addr) = addressing {
                        register_addressing_dependency(&mut dependents, &name, addr);
                    }
                    if let Some(ref pv) = pvalue {
                        dependents.entry(pv.clone()).or_default().push(name.clone());
                    }
                    if let Some(ref pm) = p_max {
                        dependents.entry(pm.clone()).or_default().push(name.clone());
                    }
                    if let Some(ref pm) = p_min {
                        dependents.entry(pm.clone()).or_default().push(name.clone());
                    }
                    for (selector, _) in &selected_if {
                        dependents
                            .entry(selector.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    let node = IntegerNode {
                        name: name.clone(),
                        addressing,
                        len,
                        access,
                        min,
                        max,
                        inc,
                        unit,
                        bitfield,
                        selectors,
                        selected_if,
                        pvalue,
                        p_max,
                        p_min,
                        value,
                        cache: std::cell::RefCell::new(None),
                        raw_cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::Integer(node));
                }
                NodeDecl::Float {
                    name,
                    addressing,
                    access,
                    min,
                    max,
                    unit,
                    scale,
                    offset,
                    selectors,
                    selected_if,
                    pvalue,
                } => {
                    if let Some(ref addr) = addressing {
                        register_addressing_dependency(&mut dependents, &name, addr);
                    }
                    if let Some(ref pv) = pvalue {
                        dependents.entry(pv.clone()).or_default().push(name.clone());
                    }
                    for (selector, _) in &selected_if {
                        dependents
                            .entry(selector.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    let node = FloatNode {
                        name: name.clone(),
                        addressing,
                        access,
                        min,
                        max,
                        unit,
                        scale,
                        offset,
                        selectors,
                        selected_if,
                        pvalue,
                        cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::Float(node));
                }
                NodeDecl::Enum {
                    name,
                    addressing,
                    access,
                    entries,
                    default,
                    selectors,
                    selected_if,
                    pvalue,
                } => {
                    if let Some(ref addr) = addressing {
                        register_addressing_dependency(&mut dependents, &name, addr);
                    }
                    if let Some(ref pv) = pvalue {
                        dependents.entry(pv.clone()).or_default().push(name.clone());
                    }
                    for (selector, _) in &selected_if {
                        dependents
                            .entry(selector.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    let mut providers = Vec::new();
                    let mut provider_set = HashSet::new();
                    for entry in &entries {
                        if let EnumValueSrc::FromNode(node_name) = &entry.value {
                            dependents
                                .entry(node_name.clone())
                                .or_default()
                                .push(name.clone());
                            if provider_set.insert(node_name.clone()) {
                                providers.push(node_name.clone());
                            }
                        }
                    }
                    providers.sort();
                    let node = EnumNode {
                        name: name.clone(),
                        addressing,
                        access,
                        pvalue,
                        entries,
                        default,
                        selectors,
                        selected_if,
                        providers,
                        value_cache: std::cell::RefCell::new(None),
                        mapping_cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::Enum(node));
                }
                NodeDecl::Boolean {
                    name,
                    addressing,
                    len,
                    access,
                    bitfield,
                    selectors,
                    selected_if,
                    pvalue,
                    on_value,
                    off_value,
                } => {
                    if let Some(ref addr) = addressing {
                        register_addressing_dependency(&mut dependents, &name, addr);
                    }
                    if let Some(ref pv) = pvalue {
                        dependents.entry(pv.clone()).or_default().push(name.clone());
                    }
                    for (selector, _) in &selected_if {
                        dependents
                            .entry(selector.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    let node = BooleanNode {
                        name: name.clone(),
                        addressing,
                        len,
                        access,
                        bitfield,
                        selectors,
                        selected_if,
                        pvalue,
                        on_value,
                        off_value,
                        cache: std::cell::RefCell::new(None),
                        raw_cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::Boolean(node));
                }
                NodeDecl::Command {
                    name,
                    address,
                    len,
                    pvalue,
                    command_value,
                } => {
                    if let Some(ref pv) = pvalue {
                        dependents.entry(pv.clone()).or_default().push(name.clone());
                    }
                    let node = CommandNode {
                        name: name.clone(),
                        address,
                        len,
                        pvalue,
                        command_value,
                    };
                    nodes.insert(name, Node::Command(node));
                }
                NodeDecl::Category { name, children } => {
                    let node = CategoryNode {
                        name: name.clone(),
                        children,
                    };
                    nodes.insert(name, Node::Category(node));
                }
                NodeDecl::SwissKnife(decl) => {
                    let name = decl.name;
                    let expr = decl.expr;
                    let variables = decl.variables;
                    let output = decl.output;
                    let ast = parse_expression(&expr).map_err(|err| GenApiError::ExprParse {
                        name: name.clone(),
                        msg: err.to_string(),
                    })?;
                    let mut used = HashSet::new();
                    collect_identifiers(&ast, &mut used);
                    for ident in &used {
                        if !variables.iter().any(|(var, _)| var == ident) {
                            return Err(GenApiError::UnknownVariable {
                                name: name.clone(),
                                var: ident.clone(),
                            });
                        }
                    }
                    for (_, provider) in &variables {
                        dependents
                            .entry(provider.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    let node = SkNode {
                        name: name.clone(),
                        output,
                        ast,
                        vars: variables,
                        cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::SwissKnife(node));
                }
                NodeDecl::Converter(decl) => {
                    let name = decl.name;
                    let ast_to = parse_expression(&decl.formula_to).map_err(|err| {
                        GenApiError::ExprParse {
                            name: name.clone(),
                            msg: format!("FormulaTo: {err}"),
                        }
                    })?;
                    let ast_from = parse_expression(&decl.formula_from).map_err(|err| {
                        GenApiError::ExprParse {
                            name: name.clone(),
                            msg: format!("FormulaFrom: {err}"),
                        }
                    })?;
                    // Register dependencies for all variable providers
                    for (_, provider) in &decl.variables_to {
                        dependents
                            .entry(provider.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    for (_, provider) in &decl.variables_from {
                        if !decl.variables_to.iter().any(|(_, p)| p == provider) {
                            dependents
                                .entry(provider.clone())
                                .or_default()
                                .push(name.clone());
                        }
                    }
                    // Also depend on p_value
                    dependents
                        .entry(decl.p_value.clone())
                        .or_default()
                        .push(name.clone());
                    let node = ConverterNode {
                        name: name.clone(),
                        p_value: decl.p_value,
                        ast_to,
                        ast_from,
                        vars_to: decl.variables_to,
                        vars_from: decl.variables_from,
                        unit: decl.unit,
                        output: decl.output,
                        cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::Converter(node));
                }
                NodeDecl::IntConverter(decl) => {
                    let name = decl.name;
                    let ast_to = parse_expression(&decl.formula_to).map_err(|err| {
                        GenApiError::ExprParse {
                            name: name.clone(),
                            msg: format!("FormulaTo: {err}"),
                        }
                    })?;
                    let ast_from = parse_expression(&decl.formula_from).map_err(|err| {
                        GenApiError::ExprParse {
                            name: name.clone(),
                            msg: format!("FormulaFrom: {err}"),
                        }
                    })?;
                    for (_, provider) in &decl.variables_to {
                        dependents
                            .entry(provider.clone())
                            .or_default()
                            .push(name.clone());
                    }
                    for (_, provider) in &decl.variables_from {
                        if !decl.variables_to.iter().any(|(_, p)| p == provider) {
                            dependents
                                .entry(provider.clone())
                                .or_default()
                                .push(name.clone());
                        }
                    }
                    dependents
                        .entry(decl.p_value.clone())
                        .or_default()
                        .push(name.clone());
                    let node = IntConverterNode {
                        name: name.clone(),
                        p_value: decl.p_value,
                        ast_to,
                        ast_from,
                        vars_to: decl.variables_to,
                        vars_from: decl.variables_from,
                        unit: decl.unit,
                        cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::IntConverter(node));
                }
                NodeDecl::String(decl) => {
                    let name = decl.name;
                    register_addressing_dependency(&mut dependents, &name, &decl.addressing);
                    let node = StringNode {
                        name: name.clone(),
                        addressing: decl.addressing,
                        access: decl.access,
                        cache: std::cell::RefCell::new(None),
                    };
                    nodes.insert(name, Node::String(node));
                }
            }
        }

        Ok(NodeMap {
            version: model.version,
            nodes,
            dependents,
            generation: Cell::new(0),
        })
    }

    /// Read an integer feature value using the provided transport.
    pub fn get_integer(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        if let Some(output) = self.nodes.get(name).and_then(|node| match node {
            Node::SwissKnife(sk) => Some(sk.output),
            _ => None,
        }) {
            return match output {
                SkOutput::Integer => {
                    let node = match self.nodes.get(name) {
                        Some(Node::SwissKnife(node)) => node,
                        _ => unreachable!("node vanished during lookup"),
                    };
                    let mut stack = HashSet::new();
                    let value = self.evaluate_swissknife(node, io, &mut stack)?;
                    round_to_i64(name, value)
                }
                SkOutput::Float => Err(GenApiError::Type(name.to_string())),
            };
        }
        let node = self.get_integer_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        // Return static value if present.
        if let Some(v) = node.value {
            return Ok(v);
        }
        // Delegate to pValue node if present.
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.get_integer(&pv, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let value = if let Some(bitfield) = node.bitfield {
            let extracted = extract(&raw, bitfield).map_err(|err| map_bitops_error(name, err))?;
            interpret_bitfield_value(name, extracted, bitfield.bit_length, node.min < 0)?
        } else {
            bytes_to_i64(name, &raw)?
        };
        debug!(node = %name, raw = value, "read integer feature");
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        Ok(value)
    }

    /// Write an integer feature and update dependent caches.
    pub fn set_integer(
        &mut self,
        name: &str,
        value: i64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_integer_node(name)?;
        ensure_writable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.set_integer(&pv, value, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if value < node.min || value > node.max {
            return Err(GenApiError::Range(name.to_string()));
        }
        if let Some(inc) = node.inc {
            if inc != 0 && (value - node.min) % inc != 0 {
                return Err(GenApiError::Range(name.to_string()));
            }
        }
        if let Some(bitfield) = node.bitfield {
            let encoded = encode_bitfield_value(name, value, bitfield.bit_length, node.min < 0)?;
            let mut raw = get_raw_or_read(&node.raw_cache, io, address, len)?;
            insert(&mut raw, bitfield, encoded).map_err(|err| map_bitops_error(name, err))?;
            debug!(node = %name, raw = value, "write integer feature");
            io.write(address, &raw).map_err(|err| match err {
                GenApiError::Io(_) => err,
                other => other,
            })?;
            node.cache.replace(Some(value));
            node.raw_cache.replace(Some(raw));
        } else {
            let bytes = i64_to_bytes(name, value, len)?;
            debug!(node = %name, raw = value, "write integer feature");
            io.write(address, &bytes).map_err(|err| match err {
                GenApiError::Io(_) => err,
                other => other,
            })?;
            node.cache.replace(Some(value));
            node.raw_cache.replace(Some(bytes));
        }
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Read a floating point feature.
    pub fn get_float(&self, name: &str, io: &dyn RegisterIo) -> Result<f64, GenApiError> {
        if let Some(output) = self.nodes.get(name).and_then(|node| match node {
            Node::SwissKnife(sk) => Some(sk.output),
            _ => None,
        }) {
            return match output {
                SkOutput::Float => {
                    let node = match self.nodes.get(name) {
                        Some(Node::SwissKnife(node)) => node,
                        _ => unreachable!("node vanished during lookup"),
                    };
                    let mut stack = HashSet::new();
                    let value = self.evaluate_swissknife(node, io, &mut stack)?;
                    Ok(value)
                }
                SkOutput::Integer => self.get_integer(name, io).map(|v| v as f64),
            };
        }
        let node = self.get_float_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.get_float(&pv, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let raw_value = bytes_to_i64(name, &raw)?;
        let value = apply_scale(node, raw_value as f64);
        debug!(node = %name, raw = raw_value, value, "read float feature");
        node.cache.replace(Some(value));
        Ok(value)
    }

    /// Write a floating point feature using the scale/offset conversion.
    pub fn set_float(
        &mut self,
        name: &str,
        value: f64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_float_node(name)?;
        ensure_writable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.set_float(&pv, value, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if value < node.min || value > node.max {
            return Err(GenApiError::Range(name.to_string()));
        }
        let raw = encode_float(node, value)?;
        let bytes = i64_to_bytes(name, raw, len)?;
        debug!(node = %name, raw, value, "write float feature");
        io.write(address, &bytes).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.cache.replace(Some(value));
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Read an enumeration feature returning the symbolic entry name.
    pub fn get_enum(&self, name: &str, io: &dyn RegisterIo) -> Result<String, GenApiError> {
        let node = self.get_enum_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        // When pValue is set, read the integer from the delegate node.
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            if let Some(value) = node.value_cache.borrow().clone() {
                return Ok(value);
            }
            let raw_value = self.get_integer(&pv, io)?;
            let entry = self.lookup_enum_entry(node, raw_value, io)?;
            node.value_cache.replace(Some(entry.clone()));
            return Ok(entry);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = node.value_cache.borrow().clone() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let raw_value = bytes_to_i64(name, &raw)?;
        let entry = self.lookup_enum_entry(node, raw_value, io)?;
        debug!(node = %name, raw = raw_value, entry = %entry, "read enum feature");
        node.value_cache.replace(Some(entry.clone()));
        Ok(entry)
    }

    /// Write an enumeration entry.
    pub fn set_enum(
        &mut self,
        name: &str,
        entry: &str,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_enum_node(name)?;
        ensure_writable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let entry_decl = node
                .entries
                .iter()
                .find(|candidate| candidate.name == entry)
                .ok_or_else(|| GenApiError::EnumNoSuchEntry {
                    node: name.to_string(),
                    entry: entry.to_string(),
                })?;
            let raw_value = self.resolve_enum_entry_value(node, entry_decl, io)?;
            let entry_str = entry.to_string();
            // Re-borrow node after mutable self call.
            self.set_integer(&pv, raw_value, io)?;
            let node = self.get_enum_node(name)?;
            node.value_cache.replace(Some(entry_str));
            node.invalidate();
            self.invalidate_dependents(name);
            return Ok(());
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        let entry_decl = node
            .entries
            .iter()
            .find(|candidate| candidate.name == entry)
            .ok_or_else(|| GenApiError::EnumNoSuchEntry {
                node: name.to_string(),
                entry: entry.to_string(),
            })?;
        let raw = self.resolve_enum_entry_value(node, entry_decl, io)?;
        let bytes = i64_to_bytes(name, raw, len)?;
        debug!(node = %name, raw, entry, "write enum feature");
        io.write(address, &bytes).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.value_cache.replace(None);
        self.invalidate_dependents(name);
        Ok(())
    }

    /// List the available entry names for an enumeration feature.
    pub fn enum_entries(&self, name: &str) -> Result<Vec<String>, GenApiError> {
        let node = self.get_enum_node(name)?;
        if let Some(mapping) = node.mapping_cache.borrow().as_ref() {
            let mut names: Vec<_> = mapping.by_name.keys().cloned().collect();
            names.sort();
            names.dedup();
            return Ok(names);
        }
        let mut names: Vec<_> = node
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Read a boolean feature.
    pub fn get_bool(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError> {
        let node = self.get_bool_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let raw = self.get_integer(&pv, io)?;
            let on = node.on_value.unwrap_or(1);
            return Ok(raw == on);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let bitfield = node
            .bitfield
            .ok_or_else(|| GenApiError::Parse(format!("{name}: boolean without bitfield")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let raw_value = extract(&raw, bitfield).map_err(|err| map_bitops_error(name, err))?;
        let value = raw_value != 0;
        debug!(node = %name, raw = raw_value, value, "read boolean feature");
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        Ok(value)
    }

    /// Write a boolean feature.
    pub fn set_bool(
        &mut self,
        name: &str,
        value: bool,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_bool_node(name)?;
        ensure_writable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let on = node.on_value.unwrap_or(1);
            let off = node.off_value.unwrap_or(0);
            let raw = if value { on } else { off };
            return self.set_integer(&pv, raw, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let bitfield = node
            .bitfield
            .ok_or_else(|| GenApiError::Parse(format!("{name}: boolean without bitfield")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        let encoded = if value { 1 } else { 0 };
        let mut raw = get_raw_or_read(&node.raw_cache, io, address, len)?;
        insert(&mut raw, bitfield, encoded).map_err(|err| map_bitops_error(name, err))?;
        debug!(node = %name, raw = encoded, value, "write boolean feature");
        io.write(address, &raw).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Execute a command feature by writing a value to the command register.
    pub fn exec_command(&mut self, name: &str, io: &dyn RegisterIo) -> Result<(), GenApiError> {
        let node = self.get_command_node(name)?;
        // Determine the value to write and the target.
        let cmd_value = node.command_value.unwrap_or(1);

        if let Some(ref pv) = node.pvalue {
            // Delegate to the pValue node.
            let pv = pv.clone();
            debug!(node = %name, "execute command via pValue");
            return self.set_integer(&pv, cmd_value, io);
        }

        let address = node
            .address
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no address or pValue")))?;
        if node.len == 0 {
            return Err(GenApiError::Parse(format!(
                "command node {name} has zero length"
            )));
        }
        let data = i64_to_bytes(name, cmd_value, node.len)?;
        debug!(node = %name, "execute command");
        io.write(address, &data).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        self.invalidate_dependents(name);
        Ok(())
    }

    fn get_integer_node(&self, name: &str) -> Result<&IntegerNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Integer(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_float_node(&self, name: &str) -> Result<&FloatNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Float(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_enum_node(&self, name: &str) -> Result<&EnumNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Enum(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_bool_node(&self, name: &str) -> Result<&BooleanNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Boolean(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_command_node(&self, name: &str) -> Result<&CommandNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Command(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn ensure_selectors(
        &self,
        node_name: &str,
        rules: &[(String, Vec<String>)],
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        for (selector, allowed) in rules {
            if allowed.is_empty() {
                continue;
            }
            let current = self.get_selector_value(selector, io)?;
            if !allowed.iter().any(|value| value == &current) {
                return Err(GenApiError::Unavailable(format!(
                    "node '{node_name}' unavailable for selector '{selector}={current}'"
                )));
            }
        }
        Ok(())
    }

    fn lookup_enum_entry(
        &self,
        node: &EnumNode,
        raw_value: i64,
        io: &dyn RegisterIo,
    ) -> Result<String, GenApiError> {
        {
            let mut cache = node.mapping_cache.borrow_mut();
            if cache.is_none() {
                *cache = Some(self.build_enum_mapping(node, io)?);
            }
            if let Some(mapping) = cache.as_ref() {
                if let Some(entry) = mapping.by_value.get(&raw_value) {
                    return Ok(entry.clone());
                }
            }
            *cache = Some(self.build_enum_mapping(node, io)?);
            if let Some(mapping) = cache.as_ref() {
                if let Some(entry) = mapping.by_value.get(&raw_value) {
                    return Ok(entry.clone());
                }
            }
        }
        Err(GenApiError::EnumValueUnknown {
            node: node.name.clone(),
            value: raw_value,
        })
    }

    fn build_enum_mapping(
        &self,
        node: &EnumNode,
        io: &dyn RegisterIo,
    ) -> Result<EnumMapping, GenApiError> {
        let mut by_value = HashMap::new();
        let mut by_name = HashMap::new();

        for entry in &node.entries {
            let value = self.resolve_enum_entry_value(node, entry, io)?;
            match by_value.entry(value) {
                HashMapEntry::Vacant(slot) => {
                    slot.insert(entry.name.clone());
                }
                HashMapEntry::Occupied(existing) => {
                    warn!(
                        enum_node = %node.name,
                        value,
                        kept = %existing.get(),
                        dropped = %entry.name,
                        "duplicate enum value"
                    );
                }
            }
            by_name.insert(entry.name.clone(), value);
        }

        let mut summary: Vec<_> = by_value
            .iter()
            .map(|(value, name)| (*value, name.clone()))
            .collect();
        summary.sort_by_key(|(value, _)| *value);
        debug!(node = %node.name, entries = ?summary, "build enum mapping");

        Ok(EnumMapping { by_value, by_name })
    }

    fn resolve_enum_entry_value(
        &self,
        node: &EnumNode,
        entry: &EnumEntryDecl,
        io: &dyn RegisterIo,
    ) -> Result<i64, GenApiError> {
        match &entry.value {
            EnumValueSrc::Literal(value) => Ok(*value),
            EnumValueSrc::FromNode(provider) => {
                let value = self.get_integer(provider, io)?;
                trace!(
                    enum_node = %node.name,
                    entry = %entry.name,
                    provider = %provider,
                    value,
                    "resolved enum entry from provider"
                );
                Ok(value)
            }
        }
    }

    fn resolve_address(
        &self,
        node_name: &str,
        addressing: &Addressing,
        io: &dyn RegisterIo,
    ) -> Result<(u64, u32), GenApiError> {
        match addressing {
            Addressing::Fixed { address, len } => Ok((*address, *len)),
            Addressing::BySelector { selector, map } => {
                let value = self.get_selector_value(selector, io)?;
                if let Some((_, (address, len))) = map.iter().find(|(name, _)| name == &value) {
                    let addr = *address;
                    let len = *len;
                    debug!(
                        node = %node_name,
                        selector = %selector,
                        value = %value,
                        address = format_args!("0x{addr:X}"),
                        len,
                        "resolve address via selector"
                    );
                    Ok((addr, len))
                } else {
                    Err(GenApiError::Unavailable(format!(
                        "node '{node_name}' unavailable for selector '{selector}={value}'"
                    )))
                }
            }
            Addressing::Indirect {
                p_address_node,
                len,
            } => {
                let addr_value = self.get_integer(p_address_node, io)?;
                if addr_value <= 0 {
                    return Err(GenApiError::BadIndirectAddress {
                        name: node_name.to_string(),
                        addr: addr_value,
                    });
                }
                let addr =
                    u64::try_from(addr_value).map_err(|_| GenApiError::BadIndirectAddress {
                        name: node_name.to_string(),
                        addr: addr_value,
                    })?;
                if addr == 0 {
                    return Err(GenApiError::BadIndirectAddress {
                        name: node_name.to_string(),
                        addr: addr_value,
                    });
                }
                debug!(
                    node = %node_name,
                    source = %p_address_node,
                    address = format_args!("0x{addr:X}"),
                    len = *len,
                    "resolve address via pAddress"
                );
                Ok((addr, *len))
            }
        }
    }

    fn get_selector_value(
        &self,
        selector: &str,
        io: &dyn RegisterIo,
    ) -> Result<String, GenApiError> {
        match self.nodes.get(selector) {
            Some(Node::Enum(_)) => self.get_enum(selector, io),
            Some(Node::Boolean(_)) => Ok(self.get_bool(selector, io)?.to_string()),
            Some(Node::Integer(_)) => Ok(self.get_integer(selector, io)?.to_string()),
            Some(_) => Err(GenApiError::Parse(format!(
                "selector {selector} has unsupported type"
            ))),
            None => Err(GenApiError::NodeNotFound(selector.to_string())),
        }
    }

    fn evaluate_swissknife(
        &self,
        node: &SkNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<f64, GenApiError> {
        if let Some((value, gen)) = *node.cache.borrow() {
            if gen == self.generation.get() {
                return Ok(value);
            }
        }
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }
        let current_gen = self.generation.get();
        let result = (|| {
            let mut values: HashMap<String, f64> = HashMap::new();
            let mut inputs = Vec::new();
            for (var, provider) in &node.vars {
                let value = self.resolve_numeric(provider, io, stack)?;
                values.insert(var.clone(), value);
                inputs.push((var.clone(), value));
            }
            let mut resolver = |ident: &str| -> Result<f64, SkEvalError> {
                values
                    .get(ident)
                    .copied()
                    .ok_or_else(|| SkEvalError::UnknownVariable(ident.to_string()))
            };
            let value = match eval_ast(&node.ast, &mut resolver) {
                Ok(value) => value,
                Err(SkEvalError::UnknownVariable(var)) => {
                    return Err(GenApiError::UnknownVariable {
                        name: node.name.clone(),
                        var,
                    });
                }
                Err(SkEvalError::DivisionByZero) => {
                    return Err(GenApiError::ExprEval {
                        name: node.name.clone(),
                        msg: "division by zero".into(),
                    });
                }
                Err(SkEvalError::UnknownFunction(func)) => {
                    return Err(GenApiError::ExprEval {
                        name: node.name.clone(),
                        msg: format!("unknown function: {func}"),
                    });
                }
                Err(SkEvalError::ArityMismatch {
                    name: func,
                    expected,
                    got,
                }) => {
                    return Err(GenApiError::ExprEval {
                        name: node.name.clone(),
                        msg: format!("function {func} expects {expected} args, got {got}"),
                    });
                }
            };
            debug!(node = %node.name, inputs = ?inputs, output = value, "evaluate SwissKnife");
            Ok(value)
        })();
        stack.remove(&node.name);
        match result {
            Ok(value) => {
                node.cache.replace(Some((value, current_gen)));
                Ok(value)
            }
            Err(err) => Err(err),
        }
    }

    fn resolve_numeric(
        &self,
        provider: &str,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<f64, GenApiError> {
        match self.nodes.get(provider) {
            Some(Node::Integer(_)) => self.get_integer(provider, io).map(|v| v as f64),
            Some(Node::Float(_)) => self.get_float(provider, io),
            Some(Node::Boolean(_)) => Ok(if self.get_bool(provider, io)? {
                1.0
            } else {
                0.0
            }),
            Some(Node::Enum(_)) => self.get_enum_numeric(provider, io).map(|v| v as f64),
            Some(Node::SwissKnife(node)) => self.evaluate_swissknife(node, io, stack),
            Some(Node::Converter(node)) => self.evaluate_converter(node, io, stack),
            Some(Node::IntConverter(node)) => self
                .evaluate_int_converter(node, io, stack)
                .map(|v| v as f64),
            Some(_) => Err(GenApiError::Type(provider.to_string())),
            None => Err(GenApiError::NodeNotFound(provider.to_string())),
        }
    }

    fn get_enum_numeric(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        let entry = self.get_enum(name, io)?;
        let node = self.get_enum_node(name)?;
        {
            let mut mapping = node.mapping_cache.borrow_mut();
            if mapping.is_none() {
                *mapping = Some(self.build_enum_mapping(node, io)?);
            }
            if let Some(map) = mapping.as_ref() {
                if let Some(value) = map.by_name.get(&entry) {
                    return Ok(*value);
                }
            }
        }
        Err(GenApiError::EnumNoSuchEntry {
            node: name.to_string(),
            entry,
        })
    }

    fn invalidate_dependents(&self, name: &str) {
        self.bump_generation();
        if let Some(children) = self.dependents.get(name) {
            let mut visited = HashSet::new();
            for child in children {
                self.invalidate_recursive(child, &mut visited);
            }
        }
    }

    fn invalidate_recursive(&self, name: &str, visited: &mut HashSet<String>) {
        if !visited.insert(name.to_string()) {
            return;
        }
        if let Some(node) = self.nodes.get(name) {
            node.invalidate_cache();
        }
        if let Some(children) = self.dependents.get(name) {
            for child in children {
                self.invalidate_recursive(child, visited);
            }
        }
    }

    fn bump_generation(&self) {
        let current = self.generation.get();
        self.generation.set(current.wrapping_add(1));
    }

    // ========================================================================
    // Converter/IntConverter/String support
    // ========================================================================

    fn get_converter_node(&self, name: &str) -> Result<&ConverterNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Converter(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_int_converter_node(&self, name: &str) -> Result<&IntConverterNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::IntConverter(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_string_node(&self, name: &str) -> Result<&StringNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::String(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    /// Read a Converter feature value (float) using the provided transport.
    pub fn get_converter(&self, name: &str, io: &dyn RegisterIo) -> Result<f64, GenApiError> {
        let node = self.get_converter_node(name)?;
        if let Some((value, gen)) = *node.cache.borrow() {
            if gen == self.generation.get() {
                return Ok(value);
            }
        }
        let mut stack = HashSet::new();
        let value = self.evaluate_converter(node, io, &mut stack)?;
        node.cache.replace(Some((value, self.generation.get())));
        Ok(value)
    }

    /// Read an IntConverter feature value (integer) using the provided transport.
    pub fn get_int_converter(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        let node = self.get_int_converter_node(name)?;
        if let Some((value, gen)) = *node.cache.borrow() {
            if gen == self.generation.get() {
                return Ok(value);
            }
        }
        let mut stack = HashSet::new();
        let value = self.evaluate_int_converter(node, io, &mut stack)?;
        node.cache.replace(Some((value, self.generation.get())));
        Ok(value)
    }

    /// Read a String feature value using the provided transport.
    pub fn get_string(&self, name: &str, io: &dyn RegisterIo) -> Result<String, GenApiError> {
        let node = self.get_string_node(name)?;
        ensure_readable(&node.access, name)?;
        if let Some((ref value, gen)) = *node.cache.borrow() {
            if gen == self.generation.get() {
                return Ok(value.clone());
            }
        }
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        let raw = io.read(address, len as usize)?;
        // Convert bytes to string, stopping at first null byte
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let value = String::from_utf8_lossy(&raw[..end]).to_string();
        node.cache
            .replace(Some((value.clone(), self.generation.get())));
        debug!(node = %name, value = %value, "get_string");
        Ok(value)
    }

    /// Write a String feature value using the provided transport.
    pub fn set_string(
        &self,
        name: &str,
        value: &str,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_string_node(name)?;
        ensure_writable(&node.access, name)?;
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        // Build byte buffer with null termination
        let mut buf = vec![0u8; len as usize];
        let bytes = value.as_bytes();
        let copy_len = bytes.len().min(len as usize);
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        io.write(address, &buf)?;
        node.cache
            .replace(Some((value.to_string(), self.generation.get())));
        self.invalidate_dependents(name);
        debug!(node = %name, value = %value, "set_string");
        Ok(())
    }

    fn evaluate_converter(
        &self,
        node: &ConverterNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<f64, GenApiError> {
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }

        let result = (|| {
            // Build variable map for formula evaluation
            let mut values: HashMap<String, f64> = HashMap::new();
            for (var, provider) in &node.vars_to {
                let value = self.resolve_numeric(provider, io, stack)?;
                values.insert(var.clone(), value);
            }
            // Evaluate the formula
            let mut resolver = |ident: &str| -> Result<f64, SkEvalError> {
                values
                    .get(ident)
                    .copied()
                    .ok_or_else(|| SkEvalError::UnknownVariable(ident.to_string()))
            };
            match eval_ast(&node.ast_to, &mut resolver) {
                Ok(value) => {
                    debug!(node = %node.name, value, "evaluate Converter");
                    Ok(value)
                }
                Err(SkEvalError::UnknownVariable(var)) => Err(GenApiError::UnknownVariable {
                    name: node.name.clone(),
                    var,
                }),
                Err(SkEvalError::DivisionByZero) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: "division by zero".into(),
                }),
                Err(SkEvalError::UnknownFunction(func)) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: format!("unknown function: {func}"),
                }),
                Err(SkEvalError::ArityMismatch {
                    name: func,
                    expected,
                    got,
                }) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: format!("function {func} expects {expected} args, got {got}"),
                }),
            }
        })();

        stack.remove(&node.name);
        result
    }

    fn evaluate_int_converter(
        &self,
        node: &IntConverterNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<i64, GenApiError> {
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }

        let result = (|| {
            let mut values: HashMap<String, f64> = HashMap::new();
            for (var, provider) in &node.vars_to {
                let value = self.resolve_numeric(provider, io, stack)?;
                values.insert(var.clone(), value);
            }
            let mut resolver = |ident: &str| -> Result<f64, SkEvalError> {
                values
                    .get(ident)
                    .copied()
                    .ok_or_else(|| SkEvalError::UnknownVariable(ident.to_string()))
            };
            match eval_ast(&node.ast_to, &mut resolver) {
                Ok(value) => {
                    let int_value = round_to_i64(&node.name, value)?;
                    debug!(node = %node.name, int_value, "evaluate IntConverter");
                    Ok(int_value)
                }
                Err(SkEvalError::UnknownVariable(var)) => Err(GenApiError::UnknownVariable {
                    name: node.name.clone(),
                    var,
                }),
                Err(SkEvalError::DivisionByZero) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: "division by zero".into(),
                }),
                Err(SkEvalError::UnknownFunction(func)) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: format!("unknown function: {func}"),
                }),
                Err(SkEvalError::ArityMismatch {
                    name: func,
                    expected,
                    got,
                }) => Err(GenApiError::ExprEval {
                    name: node.name.clone(),
                    msg: format!("function {func} expects {expected} args, got {got}"),
                }),
            }
        })();

        stack.remove(&node.name);
        result
    }
}

impl From<XmlModel> for NodeMap {
    fn from(model: XmlModel) -> Self {
        NodeMap::try_from_xml(model).expect("invalid GenApi model")
    }
}
