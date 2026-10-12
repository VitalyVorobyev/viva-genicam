//! Error types for GenApi operations.

use std::fmt;

use thiserror::Error;

use crate::swissknife::{EvalError, ParseError, Value};

/// The error a transport's failure is carried as: [`RegisterIo`] is
/// implemented outside this crate, over GVCP, USB or a test's memory map, so
/// its error type is not part of this crate's API.
///
/// [`RegisterIo`]: crate::RegisterIo
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Which way an access was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessDirection {
    /// Reading the value.
    Read,
    /// Writing the value.
    Write,
}

impl fmt::Display for AccessDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AccessDirection::Read => "read",
            AccessDirection::Write => "written",
        })
    }
}

/// How a value falls outside what a node accepts.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum RangeViolation {
    /// Below the minimum in force.
    BelowMin(Value),
    /// Above the maximum in force.
    AboveMax(Value),
    /// Not the minimum plus a whole number of increments.
    OffIncrement {
        /// The minimum the increments count from (zero without one).
        base: i64,
        /// The increment.
        inc: i64,
    },
    /// The node's register cannot hold it: too wide for its bytes, beyond an
    /// IEEE `f32`, not finite, or not a whole raw value at the node's scale.
    Unrepresentable,
}

impl fmt::Display for RangeViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RangeViolation::BelowMin(min) => write!(f, "is below the minimum {min}"),
            RangeViolation::AboveMax(max) => write!(f, "is above the maximum {max}"),
            RangeViolation::OffIncrement { base, inc } => {
                write!(f, "is not {base} plus a multiple of {inc}")
            }
            RangeViolation::Unrepresentable => f.write_str("cannot be held by its register"),
        }
    }
}

/// Error type produced by GenApi operations.
///
/// Every variant names the node it is about. A transport's failure is the
/// [`source`](std::error::Error::source) of [`GenApiError::Io`], and a
/// formula's of [`GenApiError::ExprParse`] and [`GenApiError::ExprEval`]:
/// carried, never flattened into text (ADR-0030).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GenApiError {
    /// The requested node does not exist in the nodemap.
    #[error("node not found: {0}")]
    NodeNotFound(String),
    /// The node is of another kind than the operation needs.
    #[error("{name} is a{} {kind}, not {expected}", article(kind))]
    Type {
        /// The node.
        name: String,
        /// Its kind, as [`crate::Node::kind_name`] names it.
        kind: &'static str,
        /// What the operation needs, such as `"an Integer"`.
        expected: &'static str,
    },
    /// The node's access mode forbids the operation: a read of a `WO` node, a
    /// write of an `RO` one.
    #[error("{name} cannot be {refused}: its access mode forbids it")]
    Access {
        /// The node.
        name: String,
        /// The direction refused.
        refused: AccessDirection,
    },
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
    /// Text given for a feature does not parse as the feature's type: see
    /// [`crate::NodeMap::set_text`]. Nothing was written.
    #[error("invalid value for {name}: expected {expected}, got {text:?}")]
    InvalidText {
        /// The feature.
        name: String,
        /// What the feature takes, such as `"an integer"`.
        expected: &'static str,
        /// The text as given.
        text: String,
    },
    /// The value is outside what the node accepts. Nothing was written.
    #[error("{name}: {value} {violation}")]
    Range {
        /// The node.
        name: String,
        /// The value refused.
        value: Value,
        /// How it falls outside.
        violation: RangeViolation,
    },
    /// Bytes written to a `<Register>` are not its length. Nothing was
    /// written.
    #[error("register {name} is {len} bytes; got {got}")]
    RegisterLength {
        /// The register.
        name: String,
        /// Its length.
        len: u32,
        /// The bytes given.
        got: usize,
    },
    /// The node is currently not available: its `pIsAvailable` reads false.
    #[error("node unavailable: {0}")]
    Unavailable(String),
    /// The node has no value of its own to read or write: no register, no
    /// `<pValue>` and no `<Value>`.
    #[error("{name} declares no register, <pValue> or <Value>")]
    NoValueSource {
        /// The node.
        name: String,
    },
    /// Register I/O failed in the transport. The source is the transport's
    /// error: a GVCP status, a USB failure, a timeout.
    #[error("register I/O failed: {0}")]
    Io(#[source] BoxError),
    /// A register's bytes cannot be converted to or from the node's value: an
    /// empty payload, a width its encoding has no form for (an IEEE float of
    /// three bytes), an unsigned bit field beyond `i64`.
    ///
    /// The bytes are the device's, or the width the node declares; the XML
    /// itself parsed (backlog GA-30).
    #[error("{name}: cannot convert {width} register bytes: {problem}")]
    RegisterEncoding {
        /// The node.
        name: String,
        /// The register width in bytes, or the bit-field width in bits for a
        /// bit field.
        width: usize,
        /// What does not convert, such as `"an IEEE float is 4 or 8 bytes"`.
        problem: &'static str,
    },
    /// The node's declaration cannot be evaluated: the XML loaded, but
    /// declares something a node of its kind cannot have, such as a
    /// `<Boolean>` over a register with no bit field, or a zero scale.
    #[error("{name} {problem}")]
    Declaration {
        /// The node.
        name: String,
        /// What is wrong, such as `"has a zero <Scale> numerator"`.
        problem: &'static str,
    },
    /// A declaration this crate has no builder for.
    ///
    /// [`viva_genapi_xml::NodeDecl`] is `#[non_exhaustive]`, so a node type
    /// added to the XML layer no longer breaks this crate's *build*. It
    /// surfaces here instead, at nodemap construction, and lands in
    /// [`crate::NodeMap::skipped`] where the corpus test will see it.
    #[error("node {name} has declaration kind {kind}, which this nodemap cannot build")]
    Unsupported {
        /// The node.
        name: String,
        /// The declaration's kind, as [`viva_genapi_xml::NodeDecl::kind`]
        /// names it.
        kind: &'static str,
    },
    /// Parsing a formula failed.
    #[error("failed to parse {name}'s formula {formula:?}: {source}")]
    ExprParse {
        /// The formula node.
        name: String,
        /// The formula, constant or expression as the document writes it.
        formula: String,
        /// The parser's error, with the offset into `formula`.
        #[source]
        source: ParseError,
    },
    /// Evaluating a formula failed.
    #[error("failed to evaluate {name}: {source}")]
    ExprEval {
        /// The formula node, or the node whose value the failure was found
        /// on.
        name: String,
        /// What failed.
        #[source]
        source: EvalError,
    },
    /// A formula referenced an unknown variable.
    #[error("unknown variable '{var}' referenced by {name}")]
    UnknownVariable {
        /// The formula node.
        name: String,
        /// The variable.
        var: String,
    },
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
    EnumValueUnknown {
        /// The enumeration.
        node: String,
        /// The value read.
        value: i64,
    },
    /// Attempted to select an enum entry that does not exist.
    #[error("enum {node} has no entry named {entry}")]
    EnumNoSuchEntry {
        /// The enumeration.
        node: String,
        /// The entry asked for.
        entry: String,
    },
    /// The node reads a chunk or event port, and nothing is attached to it.
    ///
    /// A chunk port's registers address one chunk of a stream buffer, an
    /// event port's one event (GenICam Standard v2.1.1 §2.8.16): there is no
    /// device memory behind them to fall back on. Attach a frame's chunks or a
    /// received event first ([`crate::NodeMap::attach_chunks`],
    /// [`crate::NodeMap::attach_event`]).
    #[error("{node} reads port {port}, and no chunk or event data is attached to it")]
    NotAttached {
        /// The register that was read.
        node: String,
        /// Its `<pPort>`.
        port: String,
    },
    /// The node is on a chunk or event port, which holds data the camera sent
    /// and cannot be written.
    #[error("{node} is on port {port}, which holds data the camera sent and cannot be written")]
    PortNotWritable {
        /// The register that was written.
        node: String,
        /// Its `<pPort>`.
        port: String,
    },
    /// The register is on a port the document does not declare, so there is
    /// nowhere to read it from.
    #[error("{node} is on port {port}, which the document does not declare")]
    UnknownPort {
        /// The register.
        node: String,
        /// Its `<pPort>`.
        port: String,
    },
    /// A device address was asked of a register on a chunk or event port,
    /// whose address counts from the start of a chunk or an event, not from
    /// device memory: see [`crate::NodeMap::register_address`].
    #[error("{node} is on port {port}: its address is relative to a chunk or an event")]
    NotDeviceMemory {
        /// The register.
        node: String,
        /// Its `<pPort>`.
        port: String,
    },
    /// The register lies outside the chunk or event data attached to its port.
    #[error("{node} reads {len} bytes at offset {offset} of port {port}, which holds {size} bytes")]
    PortOutOfRange {
        /// The register that was read.
        node: String,
        /// Its `<pPort>`.
        port: String,
        /// The resolved offset; negative on a chunk port counts from the end.
        offset: i64,
        /// Bytes the register spans.
        len: u32,
        /// Bytes attached to the port.
        size: usize,
    },
    /// Indirect addressing resolved to an invalid register.
    #[error("node {name} resolved invalid indirect address {addr:#X}")]
    BadIndirectAddress {
        /// The node.
        name: String,
        /// The address its `<pAddress>` or `<pIndex>` resolved to.
        addr: i64,
    },
    /// Bitfield metadata exceeded the backing register width.
    #[error(
        "bitfield for node {name} exceeds register length {len} (offset {bit_offset}, length {bit_length})"
    )]
    BitfieldOutOfRange {
        /// The node.
        name: String,
        /// The bit field's first bit.
        bit_offset: u16,
        /// Its width in bits.
        bit_length: u16,
        /// The register's length in bytes.
        len: usize,
    },
    /// Provided value does not fit into the declared bitfield.
    #[error("value {value} too wide for {bit_length}-bit field on node {name}")]
    ValueTooWide {
        /// The node.
        name: String,
        /// The value refused.
        value: i64,
        /// The bit field's width.
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
    MaskedWriteUnreadable {
        /// The node.
        name: String,
        /// Its register's address.
        address: u64,
    },
}

impl GenApiError {
    /// A transport's failure: `err` becomes the [`GenApiError::Io`] source.
    ///
    /// For [`RegisterIo`](crate::RegisterIo) implementations, so the error a
    /// caller sees can be downcast to the transport's own.
    pub fn io(err: impl Into<BoxError>) -> Self {
        GenApiError::Io(err.into())
    }

    pub(crate) fn access(name: &str, refused: AccessDirection) -> Self {
        GenApiError::Access {
            name: name.to_string(),
            refused,
        }
    }

    pub(crate) fn range(name: &str, value: Value, violation: RangeViolation) -> Self {
        GenApiError::Range {
            name: name.to_string(),
            value,
            violation,
        }
    }

    pub(crate) fn encoding(name: &str, width: usize, problem: &'static str) -> Self {
        GenApiError::RegisterEncoding {
            name: name.to_string(),
            width,
            problem,
        }
    }

    pub(crate) fn declaration(name: &str, problem: &'static str) -> Self {
        GenApiError::Declaration {
            name: name.to_string(),
            problem,
        }
    }

    pub(crate) fn eval(name: &str, source: EvalError) -> Self {
        GenApiError::ExprEval {
            name: name.to_string(),
            source,
        }
    }

    pub(crate) fn no_value_source(name: &str) -> Self {
        GenApiError::NoValueSource {
            name: name.to_string(),
        }
    }

    pub(crate) fn wrong_kind(name: &str, kind: &'static str, expected: &'static str) -> Self {
        GenApiError::Type {
            name: name.to_string(),
            kind,
            expected,
        }
    }
}

/// `"n"` before a kind that starts with a vowel, so the message reads
/// "an Integer", "a Float".
fn article(kind: &str) -> &'static str {
    match kind.as_bytes().first() {
        Some(b'A' | b'E' | b'I' | b'O' | b'U') => "n",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_error_reads_as_a_sentence() {
        let err = GenApiError::wrong_kind("Gain", "Float", "an Integer");
        assert_eq!(err.to_string(), "Gain is a Float, not an Integer");
        let err = GenApiError::wrong_kind("Mode", "Enumeration", "a Float");
        assert_eq!(err.to_string(), "Mode is an Enumeration, not a Float");
    }
}
