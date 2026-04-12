//! Node type definitions for the GenApi node system.

use std::cell::RefCell;
use std::collections::HashMap;

use viva_genapi_xml::{AccessMode, Addressing, BitField, ByteOrder, EnumEntryDecl, FloatEncoding};
pub use viva_genapi_xml::{NodeMeta, PredicateRefs, Representation, SkOutput, Visibility};

use crate::swissknife::AstNode;

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
    /// Converter transforming raw values to/from float values via formulas.
    Converter(ConverterNode),
    /// IntConverter transforming raw values to/from integer values via formulas.
    IntConverter(IntConverterNode),
    /// StringReg for string-typed register access.
    String(StringNode),
}

impl Node {
    /// Return the GenICam node type name (e.g. "Integer", "Float", "Enumeration").
    pub fn kind_name(&self) -> &'static str {
        match self {
            Node::Integer(_) => "Integer",
            Node::Float(_) => "Float",
            Node::Enum(_) => "Enumeration",
            Node::Boolean(_) => "Boolean",
            Node::Command(_) => "Command",
            Node::Category(_) => "Category",
            Node::SwissKnife(_) => "SwissKnife",
            Node::Converter(_) => "Converter",
            Node::IntConverter(_) => "IntConverter",
            Node::String(_) => "StringReg",
        }
    }

    /// Return the access mode of the node, if applicable.
    pub fn access_mode(&self) -> Option<viva_genapi_xml::AccessMode> {
        match self {
            Node::Integer(n) => Some(n.access),
            Node::Float(n) => Some(n.access),
            Node::Enum(n) => Some(n.access),
            Node::Boolean(n) => Some(n.access),
            Node::Command(_) => Some(viva_genapi_xml::AccessMode::WO),
            Node::Category(_) => None,
            Node::SwissKnife(_) => Some(viva_genapi_xml::AccessMode::RO),
            Node::Converter(_) => Some(viva_genapi_xml::AccessMode::RO),
            Node::IntConverter(_) => Some(viva_genapi_xml::AccessMode::RO),
            Node::String(n) => Some(n.access),
        }
    }

    /// Return the node name.
    pub fn name(&self) -> &str {
        match self {
            Node::Integer(n) => &n.name,
            Node::Float(n) => &n.name,
            Node::Enum(n) => &n.name,
            Node::Boolean(n) => &n.name,
            Node::Command(n) => &n.name,
            Node::Category(n) => &n.name,
            Node::SwissKnife(n) => &n.name,
            Node::Converter(n) => &n.name,
            Node::IntConverter(n) => &n.name,
            Node::String(n) => &n.name,
        }
    }

    /// Return the shared metadata for this node.
    pub fn meta(&self) -> &NodeMeta {
        match self {
            Node::Integer(n) => &n.meta,
            Node::Float(n) => &n.meta,
            Node::Enum(n) => &n.meta,
            Node::Boolean(n) => &n.meta,
            Node::Command(n) => &n.meta,
            Node::Category(n) => &n.meta,
            Node::SwissKnife(n) => &n.meta,
            Node::Converter(n) => &n.meta,
            Node::IntConverter(n) => &n.meta,
            Node::String(n) => &n.meta,
        }
    }

    /// Return the visibility level of this node.
    pub fn visibility(&self) -> Visibility {
        self.meta().visibility
    }

    /// Return the description of this node, if any.
    pub fn description(&self) -> Option<&str> {
        self.meta().description.as_deref()
    }

    /// Return the tooltip of this node, if any.
    pub fn tooltip(&self) -> Option<&str> {
        self.meta().tooltip.as_deref()
    }

    /// Return the display name of this node, if any.
    pub fn display_name(&self) -> Option<&str> {
        self.meta().display_name.as_deref()
    }

    /// Return the recommended representation for this node, if any.
    pub fn representation(&self) -> Option<Representation> {
        self.meta().representation
    }

    /// Return the predicate references (`pIsImplemented`, `pIsAvailable`,
    /// `pIsLocked`) declared on this node.
    pub fn predicates(&self) -> &PredicateRefs {
        match self {
            Node::Integer(n) => &n.predicates,
            Node::Float(n) => &n.predicates,
            Node::Enum(n) => &n.predicates,
            Node::Boolean(n) => &n.predicates,
            Node::Command(n) => &n.predicates,
            Node::Category(n) => &n.predicates,
            Node::SwissKnife(n) => &n.predicates,
            Node::Converter(n) => &n.predicates,
            Node::IntConverter(n) => &n.predicates,
            Node::String(n) => &n.predicates,
        }
    }

    pub(crate) fn invalidate_cache(&self) {
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
            Node::Converter(node) => {
                node.cache.replace(None);
            }
            Node::IntConverter(node) => {
                node.cache.replace(None);
            }
            Node::String(node) => {
                node.cache.replace(None);
            }
            Node::Command(_) | Node::Category(_) => {}
        }
    }
}

/// Integer feature metadata extracted from the XML description.
#[derive(Debug)]
pub struct IntegerNode {
    /// Unique feature name.
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Register addressing metadata (absent when delegated via `pvalue`).
    pub addressing: Option<Addressing>,
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
    /// Node providing the value (delegates read/write).
    pub pvalue: Option<String>,
    /// Node providing the dynamic maximum.
    pub p_max: Option<String>,
    /// Node providing the dynamic minimum.
    pub p_min: Option<String>,
    /// Static value for constant nodes.
    pub value: Option<i64>,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    pub(crate) cache: RefCell<Option<i64>>,
    pub(crate) raw_cache: RefCell<Option<Vec<u8>>>,
}

/// Floating point feature metadata.
#[derive(Debug)]
pub struct FloatNode {
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Register addressing metadata (absent when delegated via `pvalue`).
    pub addressing: Option<Addressing>,
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
    /// Node providing the value (delegates read/write).
    pub pvalue: Option<String>,
    /// How the register payload is encoded (IEEE 754 or scaled integer).
    pub encoding: FloatEncoding,
    /// Byte order of the register payload.
    pub byte_order: ByteOrder,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    pub(crate) cache: RefCell<Option<f64>>,
}

/// Enumeration feature metadata and mapping tables.
#[derive(Debug)]
pub struct EnumNode {
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Register addressing metadata (absent when delegated via `pvalue`).
    pub addressing: Option<Addressing>,
    pub access: AccessMode,
    /// Node providing the integer value (delegates register read/write).
    pub pvalue: Option<String>,
    pub entries: Vec<EnumEntryDecl>,
    pub default: Option<String>,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
    pub providers: Vec<String>,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    pub(crate) value_cache: RefCell<Option<String>>,
    pub(crate) mapping_cache: RefCell<Option<EnumMapping>>,
}

#[derive(Debug, Clone)]
pub(crate) struct EnumMapping {
    pub by_value: HashMap<i64, String>,
    pub by_name: HashMap<String, i64>,
}

impl EnumNode {
    pub(crate) fn invalidate(&self) {
        self.value_cache.replace(None);
        self.mapping_cache.replace(None);
    }
}

/// Boolean feature metadata.
#[derive(Debug)]
pub struct BooleanNode {
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Register addressing metadata (absent when delegated via `pvalue`).
    pub addressing: Option<Addressing>,
    pub len: u32,
    pub access: AccessMode,
    /// Optional bitfield (absent for pValue-backed booleans).
    pub bitfield: Option<BitField>,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
    /// Node providing the value (delegates read/write).
    pub pvalue: Option<String>,
    /// On value for pValue-backed booleans.
    pub on_value: Option<i64>,
    /// Off value for pValue-backed booleans.
    pub off_value: Option<i64>,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    pub(crate) cache: RefCell<Option<bool>>,
    pub(crate) raw_cache: RefCell<Option<Vec<u8>>>,
}

/// SwissKnife node evaluating an arithmetic expression referencing other nodes.
///
/// Integer outputs follow round-to-nearest semantics with ties towards zero
/// after the expression has been evaluated as `f64`.
#[derive(Debug)]
pub struct SkNode {
    /// Unique feature name.
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Desired output type as declared in the XML.
    pub output: SkOutput,
    /// Parsed expression AST.
    pub ast: AstNode,
    /// Mapping of variable identifiers to provider node names.
    pub vars: Vec<(String, String)>,
    /// Predicate refs gating implementation / availability.
    pub predicates: PredicateRefs,
    /// Cached value alongside the generation it was computed in.
    pub cache: RefCell<Option<(f64, u64)>>,
}

/// Command feature metadata.
#[derive(Debug)]
pub struct CommandNode {
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Fixed register address (absent when delegated via `pvalue`).
    pub address: Option<u64>,
    pub len: u32,
    /// Node providing the command register.
    pub pvalue: Option<String>,
    /// Value to write when executing the command.
    pub command_value: Option<i64>,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
}

/// Category node describing child feature names.
#[derive(Debug)]
pub struct CategoryNode {
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    pub children: Vec<String>,
    /// Predicate refs gating implementation / availability.
    pub predicates: PredicateRefs,
}

/// Converter node transforming raw values to/from float values via formulas.
///
/// Converters use two formulas:
/// - `formula_to`: converts from raw register value to user-facing float (reading)
/// - `formula_from`: converts from user-facing float to raw register value (writing)
#[derive(Debug)]
pub struct ConverterNode {
    /// Unique feature name.
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Name of the node providing the raw register value.
    pub p_value: String,
    /// Parsed AST for the formula converting raw → user value.
    pub ast_to: AstNode,
    /// Parsed AST for the formula converting user → raw value.
    pub ast_from: AstNode,
    /// Variable mappings for formula_to (reading).
    pub vars_to: Vec<(String, String)>,
    /// Variable mappings for formula_from (writing).
    pub vars_from: Vec<(String, String)>,
    /// Optional engineering unit.
    pub unit: Option<String>,
    /// Desired output type.
    pub output: SkOutput,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    /// Cached user-facing value alongside the generation it was computed in.
    pub cache: RefCell<Option<(f64, u64)>>,
}

/// IntConverter node transforming raw values to/from integer values via formulas.
#[derive(Debug)]
pub struct IntConverterNode {
    /// Unique feature name.
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Name of the node providing the raw register value.
    pub p_value: String,
    /// Parsed AST for the formula converting raw → user value.
    pub ast_to: AstNode,
    /// Parsed AST for the formula converting user → raw value.
    pub ast_from: AstNode,
    /// Variable mappings for formula_to (reading).
    pub vars_to: Vec<(String, String)>,
    /// Variable mappings for formula_from (writing).
    pub vars_from: Vec<(String, String)>,
    /// Optional engineering unit.
    pub unit: Option<String>,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    /// Cached user-facing value alongside the generation it was computed in.
    pub cache: RefCell<Option<(i64, u64)>>,
}

/// StringReg node for string-typed register access.
#[derive(Debug)]
pub struct StringNode {
    /// Unique feature name.
    pub name: String,
    /// Shared metadata (visibility, description, tooltip, etc.).
    pub meta: NodeMeta,
    /// Register addressing metadata.
    pub addressing: Addressing,
    /// Declared access rights.
    pub access: AccessMode,
    /// Predicate refs gating implementation / availability / lock state.
    pub predicates: PredicateRefs,
    /// Cached string value alongside the generation it was computed in.
    pub cache: RefCell<Option<(String, u64)>>,
}
