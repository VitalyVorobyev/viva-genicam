#![cfg_attr(docsrs, feature(doc_cfg))]
//! GenApi node system: typed feature access backed by register IO.

use std::cell::{Cell, RefCell};
use std::collections::{hash_map::Entry as HashMapEntry, HashMap, HashSet};

pub use genapi_xml::SkOutput;
use genapi_xml::{
    AccessMode, Addressing, BitField, EnumEntryDecl, EnumValueSrc, NodeDecl, XmlModel,
};
use thiserror::Error;
use tracing::{debug, trace, warn};

mod bitops;
use crate::bitops::{extract, insert, BitOpsError};
mod swissknife;
use crate::swissknife::{
    collect_identifiers, evaluate as eval_ast, parse_expression, AstNode, EvalError as SkEvalError,
};

/// Error type produced by GenApi operations.
#[derive(Debug, Error)]
pub enum GenApiError {
    /// The requested node does not exist in the nodemap.
    #[error("node not found: {0}")]
    NodeNotFound(String),
    /// The node exists but has a different type.
    #[error("type mismatch for node: {0}")]
    Type(String),
    /// The node access mode forbids the attempted operation.
    #[error("access denied for node: {0}")]
    Access(String),
    /// The provided value violates the limits declared by the node.
    #[error("range error for node: {0}")]
    Range(String),
    /// The node is currently hidden by selector state.
    #[error("node unavailable: {0}")]
    Unavailable(String),
    /// Underlying register IO failed.
    #[error("io error: {0}")]
    Io(String),
    /// Node metadata or conversion failed.
    #[error("parse error: {0}")]
    Parse(String),
    /// Parsing a SwissKnife expression failed.
    #[error("failed to parse expression for {name}: {msg}")]
    ExprParse { name: String, msg: String },
    /// Evaluating a SwissKnife expression failed at runtime.
    #[error("failed to evaluate expression for {name}: {msg}")]
    ExprEval { name: String, msg: String },
    /// A SwissKnife expression referenced an unknown variable.
    #[error("unknown variable '{var}' referenced by {name}")]
    UnknownVariable { name: String, var: String },
    /// Raw register value did not correspond to any enum entry.
    #[error("enum {node} has no entry for raw value {value}")]
    EnumValueUnknown { node: String, value: i64 },
    /// Attempted to select an enum entry that does not exist.
    #[error("enum {node} has no entry named {entry}")]
    EnumNoSuchEntry { node: String, entry: String },
    /// Indirect addressing resolved to an invalid register.
    #[error("node {name} resolved invalid indirect address {addr:#X}")]
    BadIndirectAddress { name: String, addr: i64 },
    /// Bitfield metadata exceeded the backing register width.
    #[error(
        "bitfield for node {name} exceeds register length {len} (offset {bit_offset}, length {bit_length})"
    )]
    BitfieldOutOfRange {
        name: String,
        bit_offset: u16,
        bit_length: u16,
        len: usize,
    },
    /// Provided value does not fit into the declared bitfield.
    #[error("value {value} too wide for {bit_length}-bit field on node {name}")]
    ValueTooWide {
        name: String,
        value: i64,
        bit_length: u16,
    },
}

/// Register access abstraction backed by transports such as GVCP/GenCP.
pub trait RegisterIo {
    /// Read `len` bytes starting at `addr`.
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError>;
    /// Write `data` starting at `addr`.
    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError>;
}

/// Node kinds supported by the Tier-1 subset.
#[derive(Debug)]
pub enum Node {
    /// Signed integer feature stored in a fixed-width register block.
    Integer(IntegerNode),
    /// Floating point feature with optional scale/offset conversion.
    Float(FloatNode),
    /// Enumeration feature mapping integers to symbolic names.
    Enum(EnumNode),
    /// Boolean feature represented as an integer register.
    Boolean(BooleanNode),
    /// Command feature triggering a device-side action when written.
    Command(CommandNode),
    /// Category organising related features.
    Category(CategoryNode),
    /// SwissKnife expression producing a computed value.
    SwissKnife(SkNode),
}

impl Node {
    fn invalidate_cache(&self) {
        match self {
            Node::Integer(node) => {
                node.cache.replace(None);
                node.raw_cache.replace(None);
            }
            Node::Float(node) => {
                node.cache.replace(None);
            }
            Node::Enum(node) => node.invalidate(),
            Node::Boolean(node) => {
                node.cache.replace(None);
                node.raw_cache.replace(None);
            }
            Node::SwissKnife(node) => {
                node.cache.replace(None);
            }
            Node::Command(_) | Node::Category(_) => {}
        }
    }
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

/// Integer feature metadata extracted from the XML description.
#[derive(Debug)]
pub struct IntegerNode {
    /// Unique feature name.
    pub name: String,
    /// Register addressing metadata (fixed, selector-based, or indirect).
    pub addressing: Addressing,
    /// Nominal register length in bytes.
    pub len: u32,
    /// Declared access rights.
    pub access: AccessMode,
    /// Minimum permitted user value.
    pub min: i64,
    /// Maximum permitted user value.
    pub max: i64,
    /// Optional increment step the value must respect.
    pub inc: Option<i64>,
    /// Optional engineering unit such as "us".
    pub unit: Option<String>,
    /// Optional bitfield metadata restricting active bits.
    pub bitfield: Option<BitField>,
    /// Selector nodes controlling the visibility of this node.
    pub selectors: Vec<String>,
    /// Selector gating rules in the form `(selector, allowed values)`.
    pub selected_if: Vec<(String, Vec<String>)>,
    cache: RefCell<Option<i64>>,
    raw_cache: RefCell<Option<Vec<u8>>>,
}

/// Floating point feature metadata.
#[derive(Debug)]
pub struct FloatNode {
    pub name: String,
    pub addressing: Addressing,
    pub access: AccessMode,
    pub min: f64,
    pub max: f64,
    pub unit: Option<String>,
    /// Optional rational scale `(numerator, denominator)` applied to the raw value.
    pub scale: Option<(i64, i64)>,
    /// Optional offset added after scaling.
    pub offset: Option<f64>,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
    cache: RefCell<Option<f64>>,
}

/// Enumeration feature metadata and mapping tables.
#[derive(Debug)]
pub struct EnumNode {
    pub name: String,
    pub addressing: Addressing,
    pub access: AccessMode,
    pub entries: Vec<EnumEntryDecl>,
    pub default: Option<String>,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
    pub providers: Vec<String>,
    value_cache: RefCell<Option<String>>,
    mapping_cache: RefCell<Option<EnumMapping>>,
}

#[derive(Debug, Clone)]
struct EnumMapping {
    by_value: HashMap<i64, String>,
    by_name: HashMap<String, i64>,
}

/// Boolean feature metadata.
#[derive(Debug)]
pub struct BooleanNode {
    pub name: String,
    pub addressing: Addressing,
    pub len: u32,
    pub access: AccessMode,
    pub bitfield: BitField,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
    cache: RefCell<Option<bool>>,
    raw_cache: RefCell<Option<Vec<u8>>>,
}

/// SwissKnife node evaluating an arithmetic expression referencing other nodes.
///
/// Integer outputs follow round-to-nearest semantics with ties towards zero
/// after the expression has been evaluated as `f64`.
#[derive(Debug)]
pub struct SkNode {
    /// Unique feature name.
    pub name: String,
    /// Desired output type as declared in the XML.
    pub output: SkOutput,
    /// Parsed expression AST.
    pub ast: AstNode,
    /// Mapping of variable identifiers to provider node names.
    pub vars: Vec<(String, String)>,
    /// Cached value alongside the generation it was computed in.
    pub cache: RefCell<Option<(f64, u64)>>,
}

impl EnumNode {
    fn invalidate(&self) {
        self.value_cache.replace(None);
        self.mapping_cache.replace(None);
    }
}

/// Command feature metadata.
#[derive(Debug)]
pub struct CommandNode {
    pub name: String,
    pub address: u64,
    pub len: u32,
}

/// Category node describing child feature names.
#[derive(Debug)]
pub struct CategoryNode {
    pub name: String,
    pub children: Vec<String>,
}

/// Runtime nodemap built from an [`XmlModel`] capable of reading and writing
/// feature values via a [`RegisterIo`] transport.
#[derive(Debug)]
pub struct NodeMap {
    version: String,
    nodes: HashMap<String, Node>,
    dependents: HashMap<String, Vec<String>>,
    generation: Cell<u64>,
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
                } => {
                    register_addressing_dependency(&mut dependents, &name, &addressing);
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
                        cache: RefCell::new(None),
                        raw_cache: RefCell::new(None),
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
                } => {
                    register_addressing_dependency(&mut dependents, &name, &addressing);
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
                        cache: RefCell::new(None),
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
                } => {
                    register_addressing_dependency(&mut dependents, &name, &addressing);
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
                        entries,
                        default,
                        selectors,
                        selected_if,
                        providers,
                        value_cache: RefCell::new(None),
                        mapping_cache: RefCell::new(None),
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
                } => {
                    register_addressing_dependency(&mut dependents, &name, &addressing);
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
                        cache: RefCell::new(None),
                        raw_cache: RefCell::new(None),
                    };
                    nodes.insert(name, Node::Boolean(node));
                }
                NodeDecl::Command { name, address, len } => {
                    let node = CommandNode {
                        name: name.clone(),
                        address,
                        len,
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
                        cache: RefCell::new(None),
                    };
                    nodes.insert(name, Node::SwissKnife(node));
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let raw_value = extract(&raw, node.bitfield).map_err(|err| map_bitops_error(name, err))?;
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
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        let encoded = if value { 1 } else { 0 };
        let mut raw = get_raw_or_read(&node.raw_cache, io, address, len)?;
        insert(&mut raw, node.bitfield, encoded).map_err(|err| map_bitops_error(name, err))?;
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

    /// Execute a command feature by writing a one-valued payload.
    pub fn exec_command(&mut self, name: &str, io: &dyn RegisterIo) -> Result<(), GenApiError> {
        let node = self.get_command_node(name)?;
        if node.len == 0 {
            return Err(GenApiError::Parse(format!(
                "command node {name} has zero length"
            )));
        }
        let mut data = vec![0u8; node.len as usize];
        if let Some(last) = data.last_mut() {
            *last = 1;
        }
        debug!(node = %name, "execute command");
        io.write(node.address, &data).map_err(|err| match err {
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
}

impl From<XmlModel> for NodeMap {
    fn from(model: XmlModel) -> Self {
        NodeMap::try_from_xml(model).expect("invalid GenApi model")
    }
}

fn round_to_i64(name: &str, value: f64) -> Result<i64, GenApiError> {
    if !value.is_finite() {
        return Err(GenApiError::ExprEval {
            name: name.to_string(),
            msg: "non-finite result".into(),
        });
    }
    let rounded = round_ties_to_zero(value);
    if rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
        return Err(GenApiError::ExprEval {
            name: name.to_string(),
            msg: "result out of range".into(),
        });
    }
    let truncated = rounded.trunc();
    if (rounded - truncated).abs() > 1e-9 {
        return Err(GenApiError::ExprEval {
            name: name.to_string(),
            msg: "unable to represent integer".into(),
        });
    }
    Ok(truncated as i64)
}

fn round_ties_to_zero(value: f64) -> f64 {
    if value >= 0.0 {
        let base = value.floor();
        let frac = value - base;
        if frac > 0.5 {
            base + 1.0
        } else {
            base
        }
    } else {
        let base = value.ceil();
        let frac = value - base;
        if frac < -0.5 {
            base - 1.0
        } else {
            base
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

fn bytes_to_i64(name: &str, bytes: &[u8]) -> Result<i64, GenApiError> {
    if bytes.is_empty() {
        return Err(GenApiError::Parse(format!(
            "node {name} returned empty payload"
        )));
    }
    if bytes.len() > 8 {
        return Err(GenApiError::Parse(format!(
            "node {name} uses unsupported width {}",
            bytes.len()
        )));
    }
    let mut buf = [0u8; 8];
    let offset = 8 - bytes.len();
    buf[offset..].copy_from_slice(bytes);
    if !bytes.is_empty() && (bytes[0] & 0x80) != 0 {
        for byte in &mut buf[..offset] {
            *byte = 0xFF;
        }
    }
    Ok(i64::from_be_bytes(buf))
}

fn i64_to_bytes(name: &str, value: i64, width: u32) -> Result<Vec<u8>, GenApiError> {
    if width == 0 || width > 8 {
        return Err(GenApiError::Parse(format!(
            "node {name} has unsupported width {width}"
        )));
    }
    let width = width as usize;
    let bytes = value.to_be_bytes();
    let data = bytes[8 - width..].to_vec();
    let roundtrip = bytes_to_i64(name, &data)?;
    if roundtrip != value {
        return Err(GenApiError::Range(format!(
            "value {value} does not fit {width} bytes for {name}"
        )));
    }
    Ok(data)
}

fn interpret_bitfield_value(
    name: &str,
    raw: u64,
    bit_length: u16,
    signed: bool,
) -> Result<i64, GenApiError> {
    if signed {
        Ok(sign_extend(raw, bit_length))
    } else {
        i64::try_from(raw).map_err(|_| {
            GenApiError::Parse(format!(
                "bitfield value {raw} exceeds i64 range for node {name}"
            ))
        })
    }
}

fn encode_bitfield_value(
    name: &str,
    value: i64,
    bit_length: u16,
    signed: bool,
) -> Result<u64, GenApiError> {
    if bit_length == 0 || bit_length > 64 {
        return Err(GenApiError::Parse(format!(
            "node {name} uses unsupported bitfield width {bit_length}"
        )));
    }
    if signed {
        let width = bit_length as u32;
        let min_allowed = -(1i128 << (width - 1));
        let max_allowed = (1i128 << (width - 1)) - 1;
        let value_i128 = value as i128;
        if value_i128 < min_allowed || value_i128 > max_allowed {
            return Err(GenApiError::ValueTooWide {
                name: name.to_string(),
                value,
                bit_length,
            });
        }
        let mask = mask_u128(bit_length) as i128;
        Ok((value_i128 & mask) as u64)
    } else {
        if value < 0 {
            return Err(GenApiError::ValueTooWide {
                name: name.to_string(),
                value,
                bit_length,
            });
        }
        let mask = mask_u128(bit_length);
        if (value as u128) > mask {
            return Err(GenApiError::ValueTooWide {
                name: name.to_string(),
                value,
                bit_length,
            });
        }
        Ok(value as u64)
    }
}

fn mask_u128(bit_length: u16) -> u128 {
    if bit_length == 64 {
        u64::MAX as u128
    } else {
        (1u128 << bit_length) - 1
    }
}

fn sign_extend(value: u64, bits: u16) -> i64 {
    let shift = 64 - bits as u32;
    ((value << shift) as i64) >> shift
}

/// Get raw bytes from cache or read from device for read-modify-write operations.
///
/// This helper is used when writing to a bitfield requires first reading the current
/// register value, modifying specific bits, and writing back the result.
fn get_raw_or_read(
    cache: &std::cell::RefCell<Option<Vec<u8>>>,
    io: &dyn RegisterIo,
    address: u64,
    len: u32,
) -> Result<Vec<u8>, GenApiError> {
    let cached = cache.borrow().clone();
    if let Some(bytes) = cached {
        if bytes.len() == len as usize {
            return Ok(bytes);
        }
    }
    io.read(address, len as usize).map_err(|err| match err {
        GenApiError::Io(_) => err,
        other => other,
    })
}

fn map_bitops_error(name: &str, err: BitOpsError) -> GenApiError {
    match err {
        BitOpsError::UnsupportedWidth { len } => {
            GenApiError::Parse(format!("node {name} uses unsupported register width {len}"))
        }
        BitOpsError::UnsupportedLength { bit_length } => GenApiError::Parse(format!(
            "node {name} uses unsupported bitfield length {bit_length}"
        )),
        BitOpsError::OutOfRange {
            len,
            bit_offset,
            bit_length,
        } => GenApiError::BitfieldOutOfRange {
            name: name.to_string(),
            bit_offset,
            bit_length,
            len,
        },
        BitOpsError::ValueTooWide { bit_length, value } => GenApiError::ValueTooWide {
            name: name.to_string(),
            value: i64::try_from(value).unwrap_or(i64::MAX),
            bit_length,
        },
    }
}

fn apply_scale(node: &FloatNode, raw: f64) -> f64 {
    let mut value = raw;
    if let Some((num, den)) = node.scale {
        value *= num as f64 / den as f64;
    }
    if let Some(offset) = node.offset {
        value += offset;
    }
    value
}

fn encode_float(node: &FloatNode, value: f64) -> Result<i64, GenApiError> {
    let mut raw = value;
    if let Some(offset) = node.offset {
        raw -= offset;
    }
    if let Some((num, den)) = node.scale {
        if num == 0 {
            return Err(GenApiError::Parse(format!(
                "node {} has zero scale numerator",
                node.name
            )));
        }
        raw *= den as f64 / num as f64;
    }
    let rounded = raw.round();
    if (raw - rounded).abs() > 1e-6 {
        return Err(GenApiError::Range(node.name.clone()));
    }
    let raw_i64 = rounded as i64;
    Ok(raw_i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <Max>4096</Max>
                <Inc>2</Inc>
            </Integer>
            <Float Name="ExposureTime">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>10.0</Min>
                <Max>100000.0</Max>
                <Scale>1/1000</Scale>
            </Float>
            <Enumeration Name="GainSelector">
                <Address>0x300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="All" Value="0" />
                <EnumEntry Name="Red" Value="1" />
                <EnumEntry Name="Blue" Value="2" />
            </Enumeration>
            <Integer Name="Gain">
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
                <pSelected>GainSelector</pSelected>
                <Selected>All</Selected>
                <Address>0x310</Address>
                <Selected>Red</Selected>
                <Address>0x314</Address>
                <Selected>Blue</Selected>
            </Integer>
            <Boolean Name="GammaEnable">
                <Address>0x400</Address>
                <Length>1</Length>
                <AccessMode>RW</AccessMode>
            </Boolean>
            <Command Name="AcquisitionStart">
                <Address>0x500</Address>
                <Length>4</Length>
            </Command>
        </RegisterDescription>
    "#;

    const INDIRECT_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="RegAddr">
                <Address>0x2000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
            </Integer>
            <Integer Name="Gain">
                <pAddress>RegAddr</pAddress>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>255</Max>
            </Integer>
        </RegisterDescription>
    "#;

    const ENUM_PVALUE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Enumeration Name="Mode">
                <Address>0x4000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="Fixed10">
                    <Value>10</Value>
                </EnumEntry>
                <EnumEntry Name="DynFromReg">
                    <pValue>RegModeVal</pValue>
                </EnumEntry>
            </Enumeration>
            <Integer Name="RegModeVal">
                <Address>0x4100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
            </Integer>
        </RegisterDescription>
    "#;

    const BITFIELD_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="LeByte">
                <Address>0x5000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
                <Mask>0x0000FF00</Mask>
            </Integer>
            <Integer Name="BeBits">
                <Address>0x5004</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>15</Max>
                <Lsb>13</Lsb>
                <Msb>15</Msb>
                <Endianness>BigEndian</Endianness>
            </Integer>
            <Boolean Name="PackedFlag">
                <Address>0x5006</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Bit>13</Bit>
            </Boolean>
        </RegisterDescription>
    "#;

    const SWISSKNIFE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="GainRaw">
                <Address>0x3000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>1000</Max>
            </Integer>
            <Float Name="Offset">
                <Address>0x3008</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>-100.0</Min>
                <Max>100.0</Max>
            </Float>
            <Integer Name="B">
                <Address>0x3010</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>-1000</Min>
                <Max>1000</Max>
            </Integer>
            <SwissKnife Name="ComputedGain">
                <Expression>(GainRaw * 0.5) + Offset</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="Offset">Offset</pVariable>
                <Output>Float</Output>
            </SwissKnife>
            <SwissKnife Name="DivideInt">
                <Expression>GainRaw / 3</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <Output>Integer</Output>
            </SwissKnife>
            <SwissKnife Name="Unary">
                <Expression>-GainRaw + 10</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <Output>Integer</Output>
            </SwissKnife>
            <SwissKnife Name="DivideByZero">
                <Expression>GainRaw / B</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="B">B</pVariable>
                <Output>Float</Output>
            </SwissKnife>
        </RegisterDescription>
    "#;

    #[derive(Default)]
    struct MockIo {
        regs: RefCell<HashMap<u64, Vec<u8>>>,
        reads: RefCell<HashMap<u64, usize>>,
    }

    impl MockIo {
        fn with_registers(entries: &[(u64, Vec<u8>)]) -> Self {
            let mut regs = HashMap::new();
            for (addr, data) in entries {
                regs.insert(*addr, data.clone());
            }
            MockIo {
                regs: RefCell::new(regs),
                reads: RefCell::new(HashMap::new()),
            }
        }

        fn read_count(&self, addr: u64) -> usize {
            *self.reads.borrow().get(&addr).unwrap_or(&0)
        }
    }

    impl RegisterIo for MockIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            let mut reads = self.reads.borrow_mut();
            *reads.entry(addr).or_default() += 1;
            let regs = self.regs.borrow();
            let data = regs
                .get(&addr)
                .ok_or_else(|| GenApiError::Io(format!("read miss at 0x{addr:08X}")))?;
            if data.len() != len {
                return Err(GenApiError::Io(format!(
                    "length mismatch at 0x{addr:08X}: expected {len}, have {}",
                    data.len()
                )));
            }
            Ok(data.clone())
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            self.regs.borrow_mut().insert(addr, data.to_vec());
            Ok(())
        }
    }

    fn build_nodemap() -> NodeMap {
        let model = genapi_xml::parse(FIXTURE).expect("parse fixture");
        NodeMap::from(model)
    }

    fn build_indirect_nodemap() -> NodeMap {
        let model = genapi_xml::parse(INDIRECT_FIXTURE).expect("parse indirect fixture");
        NodeMap::from(model)
    }

    fn build_enum_pvalue_nodemap() -> NodeMap {
        let model = genapi_xml::parse(ENUM_PVALUE_FIXTURE).expect("parse enum pvalue fixture");
        NodeMap::from(model)
    }

    fn build_bitfield_nodemap() -> NodeMap {
        let model = genapi_xml::parse(BITFIELD_FIXTURE).expect("parse bitfield fixture");
        NodeMap::from(model)
    }

    fn build_swissknife_nodemap() -> NodeMap {
        let model = genapi_xml::parse(SWISSKNIFE_FIXTURE).expect("parse swissknife fixture");
        NodeMap::from(model)
    }

    #[test]
    fn integer_roundtrip_and_cache() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[(0x100, vec![0, 0, 4, 0])]);
        let width = nodemap.get_integer("Width", &io).expect("read width");
        assert_eq!(width, 1024);
        assert_eq!(io.read_count(0x100), 1);
        let width_again = nodemap.get_integer("Width", &io).expect("cached width");
        assert_eq!(width_again, 1024);
        assert_eq!(io.read_count(0x100), 1, "cached value should be reused");
        nodemap
            .set_integer("Width", 1030, &io)
            .expect("write width");
        let width = nodemap
            .get_integer("Width", &io)
            .expect("read updated width");
        assert_eq!(width, 1030);
        assert_eq!(io.read_count(0x100), 1, "write should update cache");
    }

    #[test]
    fn float_conversion_roundtrip() {
        let mut nodemap = build_nodemap();
        let raw = 50_000i64; // 50 ms with 1/1000 scale
        let io = MockIo::with_registers(&[(0x200, i64_to_bytes("ExposureTime", raw, 4).unwrap())]);
        let exposure = nodemap
            .get_float("ExposureTime", &io)
            .expect("read exposure");
        assert!((exposure - 50.0).abs() < 1e-6);
        nodemap
            .set_float("ExposureTime", 75.0, &io)
            .expect("write exposure");
        let raw_back = bytes_to_i64("ExposureTime", &io.read(0x200, 4).unwrap()).unwrap();
        assert_eq!(raw_back, 75_000);
    }

    #[test]
    fn selector_address_switching() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[
            (0x300, i64_to_bytes("GainSelector", 0, 2).unwrap()),
            (0x310, i64_to_bytes("Gain", 10, 2).unwrap()),
            (0x314, i64_to_bytes("Gain", 24, 2).unwrap()),
        ]);

        let gain_all = nodemap.get_integer("Gain", &io).expect("gain for All");
        assert_eq!(gain_all, 10);
        assert_eq!(io.read_count(0x310), 1);
        assert_eq!(io.read_count(0x314), 0);

        io.write(0x314, &i64_to_bytes("Gain", 32, 2).unwrap())
            .expect("update red gain");
        nodemap
            .set_enum("GainSelector", "Red", &io)
            .expect("set selector to red");
        let gain_red = nodemap.get_integer("Gain", &io).expect("gain for Red");
        assert_eq!(gain_red, 32);
        assert_eq!(
            io.read_count(0x310),
            1,
            "previous address should not be reread"
        );
        assert_eq!(io.read_count(0x314), 1);

        let gain_red_cached = nodemap.get_integer("Gain", &io).expect("cached red");
        assert_eq!(gain_red_cached, 32);
        assert_eq!(io.read_count(0x314), 1, "selector cache should be reused");

        nodemap
            .set_enum("GainSelector", "Blue", &io)
            .expect("set selector to blue");
        let err = nodemap.get_integer("Gain", &io).unwrap_err();
        match err {
            GenApiError::Unavailable(msg) => {
                assert!(msg.contains("GainSelector=Blue"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(
            io.read_count(0x314),
            1,
            "no read expected for missing mapping"
        );

        io.write(0x310, &i64_to_bytes("Gain", 12, 2).unwrap())
            .expect("update all gain");
        nodemap
            .set_enum("GainSelector", "All", &io)
            .expect("restore selector to all");
        let gain_all_updated = nodemap
            .get_integer("Gain", &io)
            .expect("gain for All again");
        assert_eq!(gain_all_updated, 12);
        assert_eq!(
            io.read_count(0x310),
            2,
            "address switch should invalidate cache"
        );
    }

    #[test]
    fn range_enforcement() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[(0x100, vec![0, 0, 0, 16])]);
        let err = nodemap.set_integer("Width", 17, &io).unwrap_err();
        assert!(matches!(err, GenApiError::Range(_)));
    }

    #[test]
    fn command_exec() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[]);
        nodemap
            .exec_command("AcquisitionStart", &io)
            .expect("exec command");
        let payload = io.read(0x500, 4).expect("command write");
        assert_eq!(payload, vec![0, 0, 0, 1]);
    }

    #[test]
    fn indirect_address_resolution() {
        let mut nodemap = build_indirect_nodemap();
        let io = MockIo::with_registers(&[
            (0x2000, i64_to_bytes("RegAddr", 0x3000, 4).unwrap()),
            (0x3000, i64_to_bytes("Gain", 123, 4).unwrap()),
            (0x3100, i64_to_bytes("Gain", 77, 4).unwrap()),
        ]);

        let initial = nodemap.get_integer("Gain", &io).expect("read gain");
        assert_eq!(initial, 123);
        assert_eq!(io.read_count(0x2000), 1);
        assert_eq!(io.read_count(0x3000), 1);

        nodemap
            .set_integer("RegAddr", 0x3100, &io)
            .expect("set indirect address");
        let updated = nodemap
            .get_integer("Gain", &io)
            .expect("read gain after change");
        assert_eq!(updated, 77);
        assert_eq!(io.read_count(0x2000), 1);
        assert_eq!(io.read_count(0x3000), 1);
        assert_eq!(io.read_count(0x3100), 1);
    }

    #[test]
    fn indirect_bad_address() {
        let mut nodemap = build_indirect_nodemap();
        let io = MockIo::with_registers(&[(0x2000, vec![0, 0, 0, 0])]);

        nodemap
            .set_integer("RegAddr", 0, &io)
            .expect("write zero address");
        let err = nodemap.get_integer("Gain", &io).unwrap_err();
        match err {
            GenApiError::BadIndirectAddress { name, addr } => {
                assert_eq!(name, "Gain");
                assert_eq!(addr, 0);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(io.read_count(0x2000), 0);
    }

    #[test]
    fn enum_literal_entry_read() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 10, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        let value = nodemap.get_enum("Mode", &io).expect("read mode");
        assert_eq!(value, "Fixed10");
        assert_eq!(
            io.read_count(0x4100),
            1,
            "provider should be read once for mapping"
        );
    }

    #[test]
    fn enum_provider_entry_read() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 42, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        let value = nodemap.get_enum("Mode", &io).expect("read dynamic mode");
        assert_eq!(value, "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_set_uses_provider_value() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 0, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum");
        let raw = bytes_to_i64("Mode", &io.read(0x4000, 4).unwrap()).unwrap();
        assert_eq!(raw, 42);
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_provider_update_invalidates_mapping() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 42, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        assert_eq!(nodemap.get_enum("Mode", &io).unwrap(), "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);

        nodemap
            .set_integer("RegModeVal", 17, &io)
            .expect("update provider");
        io.write(0x4000, &i64_to_bytes("Mode", 0, 4).unwrap())
            .expect("reset mode register");

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum after provider change");
        let raw = bytes_to_i64("Mode", &io.read(0x4000, 4).unwrap()).unwrap();
        assert_eq!(raw, 17);
    }

    #[test]
    fn enum_unknown_value_error() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 99, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        let err = nodemap.get_enum("Mode", &io).unwrap_err();
        match err {
            GenApiError::EnumValueUnknown { node, value } => {
                assert_eq!(node, "Mode");
                assert_eq!(value, 99);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn enum_entries_are_sorted() {
        let nodemap = build_enum_pvalue_nodemap();
        let entries = nodemap.enum_entries("Mode").expect("entries");
        assert_eq!(
            entries,
            vec!["DynFromReg".to_string(), "Fixed10".to_string()]
        );
    }

    #[test]
    fn bitfield_le_integer_roundtrip() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5000, vec![0xAA, 0xBB, 0xCC, 0xDD])]);

        let value = nodemap
            .get_integer("LeByte", &io)
            .expect("read little-endian field");
        assert_eq!(value, 0xBB);

        nodemap
            .set_integer("LeByte", 0x55, &io)
            .expect("write little-endian field");
        let data = io.read(0x5000, 4).expect("read back register");
        assert_eq!(data, vec![0xAA, 0x55, 0xCC, 0xDD]);
    }

    #[test]
    fn bitfield_be_integer_roundtrip() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5004, vec![0b1010_0000, 0b0000_0000])]);

        let value = nodemap
            .get_integer("BeBits", &io)
            .expect("read big-endian bits");
        assert_eq!(value, 0b101);

        nodemap
            .set_integer("BeBits", 0b010, &io)
            .expect("write big-endian bits");
        let data = io.read(0x5004, 2).expect("read back register");
        assert_eq!(data, vec![0b0100_0000, 0b0000_0000]);
    }

    #[test]
    fn bitfield_boolean_toggle() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5006, vec![0x00, 0x20, 0x00, 0x00])]);

        assert!(nodemap.get_bool("PackedFlag", &io).expect("read flag"));

        nodemap
            .set_bool("PackedFlag", false, &io)
            .expect("clear flag");
        let data = io.read(0x5006, 4).expect("read cleared");
        assert_eq!(data, vec![0x00, 0x00, 0x00, 0x00]);

        nodemap.set_bool("PackedFlag", true, &io).expect("set flag");
        let data = io.read(0x5006, 4).expect("read set");
        assert_eq!(data, vec![0x00, 0x20, 0x00, 0x00]);
    }

    #[test]
    fn bitfield_value_too_wide() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5004, vec![0x00, 0x00])]);

        let err = nodemap
            .set_integer("BeBits", 8, &io)
            .expect_err("value too wide");
        match err {
            GenApiError::ValueTooWide {
                name, bit_length, ..
            } => {
                assert_eq!(name, "BeBits");
                assert_eq!(bit_length, 3);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
    #[test]
    fn swissknife_evaluates_and_invalidates() {
        let mut nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (0x3000, i64_to_bytes("GainRaw", 100, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 3, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 1, 4).unwrap()),
        ]);

        let value = nodemap
            .get_float("ComputedGain", &io)
            .expect("compute gain");
        assert!((value - 53.0).abs() < 1e-6);

        nodemap
            .set_integer("GainRaw", 120, &io)
            .expect("update raw gain");
        let updated = nodemap
            .get_float("ComputedGain", &io)
            .expect("recompute gain");
        assert!((updated - 63.0).abs() < 1e-6);
    }

    #[test]
    fn swissknife_integer_rounding_and_unary() {
        let mut nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (0x3000, i64_to_bytes("GainRaw", 5, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 0, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 1, 4).unwrap()),
        ]);

        let divided = nodemap
            .get_integer("DivideInt", &io)
            .expect("integer division");
        assert_eq!(divided, 2);

        nodemap
            .set_integer("GainRaw", 3, &io)
            .expect("update gain raw");
        let unary = nodemap.get_integer("Unary", &io).expect("unary expression");
        assert_eq!(unary, 7);
    }

    #[test]
    fn swissknife_unknown_variable_error() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="A">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>100</Max>
                </Integer>
                <SwissKnife Name="Bad">
                    <Expression>A + Missing</Expression>
                    <pVariable Name="A">A</pVariable>
                </SwissKnife>
            </RegisterDescription>
        "#;

        let model = genapi_xml::parse(XML).expect("parse invalid swissknife");
        let err = NodeMap::try_from_xml(model).expect_err("unknown variable");
        match err {
            GenApiError::UnknownVariable { name, var } => {
                assert_eq!(name, "Bad");
                assert_eq!(var, "Missing");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn swissknife_division_by_zero() {
        let nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (0x3000, i64_to_bytes("GainRaw", 10, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 0, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 0, 4).unwrap()),
        ]);

        let err = nodemap
            .get_float("DivideByZero", &io)
            .expect_err("division by zero");
        match err {
            GenApiError::ExprEval { name, msg } => {
                assert_eq!(name, "DivideByZero");
                assert_eq!(msg, "division by zero");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
