//! Error types for GenApi operations.

use thiserror::Error;

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
