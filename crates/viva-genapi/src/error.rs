//! Error types for GenApi operations.

use thiserror::Error;

/// Error type produced by GenApi operations.
#[derive(Debug, Error)]
#[non_exhaustive]
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
    /// The node declares `pIsLocked` and the device currently reports it
    /// locked, so the write was refused locally.
    ///
    /// Names the locking feature, because that is the actionable part: a
    /// caller told only "access denied" has nowhere to go, whereas
    /// "`ExposureTime` is locked by `ExposureTime_Lck`" points at the feature
    /// to change first. See ADR-0018 and backlog GA-06.
    #[error("node {name} is locked by {locked_by}")]
    Locked {
        /// The node whose write was refused.
        name: String,
        /// The feature named by this node's `pIsLocked`.
        locked_by: String,
    },
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
    /// A declaration this crate has no builder for.
    ///
    /// [`viva_genapi_xml::NodeDecl`] is `#[non_exhaustive]`, so a node type
    /// added to the XML layer no longer breaks this crate's *build*. It
    /// surfaces here instead, at nodemap construction, and lands in
    /// [`crate::NodeMap::skipped`] where the corpus test will see it.
    #[error("unsupported node declaration: {0}")]
    Unsupported(String),
    /// Parsing a SwissKnife expression failed.
    #[error("failed to parse expression for {name}: {msg}")]
    ExprParse { name: String, msg: String },
    /// Evaluating a SwissKnife expression failed at runtime.
    #[error("failed to evaluate expression for {name}: {msg}")]
    ExprEval { name: String, msg: String },
    /// A SwissKnife expression referenced an unknown variable.
    #[error("unknown variable '{var}' referenced by {name}")]
    UnknownVariable { name: String, var: String },
    /// A formula qualified a `<pVariable>` with an extension the GenApi
    /// standard does not define — anything but `.Value`, `.Min`, `.Max`,
    /// `.Inc` or `.Entry.<Name>`.
    #[error("{name}: variable '{var}' has unknown extension '.{extension}'")]
    UnknownVariableExtension {
        /// The formula node.
        name: String,
        /// The qualified identifier as written.
        var: String,
        /// The part after the variable name's first `.`.
        extension: String,
    },
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
    /// A bitfield write needs the rest of its register, and the register is
    /// write-only with nothing cached.
    ///
    /// Writing some bits of a register means sending the whole register, so
    /// the other bits have to come from somewhere. For a `WO` register they
    /// cannot come from the device — it refuses the read — and inventing them
    /// would silently overwrite whatever the device holds. Refused locally,
    /// before anything reaches the wire (backlog GA-31, issue #135).
    #[error(
        "cannot write bitfield node {name}: its register at {address:#X} is write-only, so the \
         other bits it shares cannot be read back, and none are cached"
    )]
    MaskedWriteUnreadable { name: String, address: u64 },
}
