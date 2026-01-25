//! Node type definitions for the GenApi node system.

use std::cell::RefCell;
use std::collections::HashMap;

pub use genapi_xml::SkOutput;
use genapi_xml::{AccessMode, Addressing, BitField, EnumEntryDecl};

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
}

impl Node {
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
            Node::Command(_) | Node::Category(_) => {}
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
    pub(crate) cache: RefCell<Option<i64>>,
    pub(crate) raw_cache: RefCell<Option<Vec<u8>>>,
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
    pub(crate) cache: RefCell<Option<f64>>,
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
    pub addressing: Addressing,
    pub len: u32,
    pub access: AccessMode,
    pub bitfield: BitField,
    pub selectors: Vec<String>,
    pub selected_if: Vec<(String, Vec<String>)>,
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
    /// Desired output type as declared in the XML.
    pub output: SkOutput,
    /// Parsed expression AST.
    pub ast: AstNode,
    /// Mapping of variable identifiers to provider node names.
    pub vars: Vec<(String, String)>,
    /// Cached value alongside the generation it was computed in.
    pub cache: RefCell<Option<(f64, u64)>>,
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
