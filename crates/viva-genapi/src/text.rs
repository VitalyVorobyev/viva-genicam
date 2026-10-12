//! Feature values as text: for a program that does not know a feature's type
//! in advance — a command line, a configuration file.

use crate::nodes::Node;
use crate::{GenApiError, NodeMap, RegisterIo, SkOutput};

impl NodeMap {
    /// Read a feature as text, formatted by its type: an integer in decimal,
    /// a float in the shortest form that reads back to the same value, an
    /// enumeration as its entry's name, a boolean as `true` or `false`.
    ///
    /// A category has no value and a command none to read: both are errors.
    /// So is a `<Register>`, a byte array with no declared interpretation;
    /// read it with [`NodeMap::get_register`].
    pub fn get_text(&self, name: &str, io: &dyn RegisterIo) -> Result<String, GenApiError> {
        match self.node(name) {
            Some(Node::Integer(_) | Node::IntConverter(_)) => {
                Ok(self.get_integer(name, io)?.to_string())
            }
            Some(Node::Float(_)) => Ok(self.get_float(name, io)?.to_string()),
            Some(Node::Enum(_)) => self.get_enum(name, io),
            Some(Node::Boolean(_)) => Ok(self.get_bool(name, io)?.to_string()),
            Some(Node::SwissKnife(sk)) => match sk.output {
                SkOutput::Float => Ok(self.get_float(name, io)?.to_string()),
                SkOutput::Integer => Ok(self.get_integer(name, io)?.to_string()),
            },
            Some(Node::Converter(conv)) => match conv.output {
                SkOutput::Float => Ok(self.get_converter(name, io)?.to_string()),
                SkOutput::Integer => Ok((self.get_converter(name, io)? as i64).to_string()),
            },
            Some(Node::String(_)) => self.get_string(name, io),
            Some(Node::Command(_)) => Err(GenApiError::wrong_kind(
                name,
                "Command",
                "a feature with a value (a command is executed)",
            )),
            Some(Node::Category(_)) => Err(GenApiError::wrong_kind(
                name,
                "Category",
                "a feature with a value",
            )),
            // A `<Register>` is a byte array with no declared interpretation:
            // rendering it here would mean inventing an encoding nothing asked
            // for. Raw access goes through `get_register`.
            Some(Node::Register(_)) => Err(GenApiError::wrong_kind(
                name,
                "Register",
                "a feature with a text form (a register is read as bytes)",
            )),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    /// Write a feature from text, parsed by its type: the counterpart of
    /// [`NodeMap::get_text`]. An integer is decimal or `0x`-prefixed
    /// hexadecimal; a boolean is `true`/`false` or `1`/`0`.
    ///
    /// Text that does not parse as the feature's type is
    /// [`GenApiError::InvalidText`], and nothing is written. A command is
    /// executed, never set, and a category has nothing to set: both are
    /// errors.
    pub fn set_text(
        &mut self,
        name: &str,
        text: &str,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let invalid = |expected: &'static str| GenApiError::InvalidText {
            name: name.to_string(),
            expected,
            text: text.to_string(),
        };
        match self.node(name) {
            // A converter writes through its `<FormulaTo>`: `set_integer` and
            // `set_float` dispatch to it.
            Some(Node::Integer(_) | Node::IntConverter(_)) => {
                let value = parse_integer(text).ok_or_else(|| invalid("an integer"))?;
                self.set_integer(name, value, io)
            }
            Some(Node::Float(_) | Node::Converter(_)) => {
                let value: f64 = text.trim().parse().map_err(|_| invalid("a number"))?;
                self.set_float(name, value, io)
            }
            Some(Node::Enum(_)) => self.set_enum(name, text.trim(), io),
            Some(Node::Boolean(_)) => {
                let value = parse_bool(text).ok_or_else(|| invalid("a boolean"))?;
                self.set_bool(name, value, io)
            }
            Some(Node::String(_)) => self.set_string(name, text, io),
            Some(Node::Command(_)) => Err(GenApiError::wrong_kind(
                name,
                "Command",
                "a feature that is set (a command is executed)",
            )),
            Some(other @ (Node::SwissKnife(_) | Node::Register(_) | Node::Category(_))) => Err(
                GenApiError::wrong_kind(name, other.kind_name(), "a feature set from text"),
            ),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }
}

/// `true`/`false` or `1`/`0`, in any case.
pub(crate) fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// Decimal, or hexadecimal after `0x`; either may be negative.
pub(crate) fn parse_integer(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let magnitude = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => i64::from_str_radix(hex, 16).ok()?,
        None => return text.parse().ok(),
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// A register's bytes as `0x` and two lowercase hexadecimal digits per byte,
/// in register order.
pub(crate) fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The inverse of [`bytes_to_hex`]; the `0x` is optional.
pub(crate) fn hex_to_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_decimal_or_hexadecimal() {
        assert_eq!(parse_integer("42"), Some(42));
        assert_eq!(parse_integer(" -7 "), Some(-7));
        assert_eq!(parse_integer("0x1F"), Some(31));
        assert_eq!(parse_integer("-0x10"), Some(-16));
        assert_eq!(parse_integer("4.5"), None);
        assert_eq!(parse_integer("0x"), None);
    }

    #[test]
    fn register_bytes_round_trip_through_hex() {
        let bytes = [0x00, 0x1f, 0xa0, 0xff];
        assert_eq!(bytes_to_hex(&bytes), "0x001fa0ff");
        assert_eq!(hex_to_bytes("0x001fa0ff").as_deref(), Some(&bytes[..]));
        assert_eq!(hex_to_bytes("001FA0FF").as_deref(), Some(&bytes[..]));
        assert_eq!(hex_to_bytes("0x123"), None);
        assert_eq!(hex_to_bytes("0xzz"), None);
    }
}
