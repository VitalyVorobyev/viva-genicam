//! Numeric conversion utilities for register values and bitfields.

use crate::GenApiError;
use crate::bitops::BitOpsError;
use crate::nodes::FloatNode;

/// Convert a big-endian byte slice (up to 8 bytes) to a signed 64-bit integer.
pub fn bytes_to_i64(name: &str, bytes: &[u8]) -> Result<i64, GenApiError> {
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

/// Convert a signed 64-bit integer to a big-endian byte vector of specified width.
pub fn i64_to_bytes(name: &str, value: i64, width: u32) -> Result<Vec<u8>, GenApiError> {
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

/// Interpret an extracted bitfield value, applying sign extension if needed.
pub fn interpret_bitfield_value(
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

/// Encode a value into bitfield representation, validating range constraints.
pub fn encode_bitfield_value(
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

/// Round a floating-point value to i64 using round-to-nearest with ties toward zero.
pub fn round_to_i64(name: &str, value: f64) -> Result<i64, GenApiError> {
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
        if frac > 0.5 { base + 1.0 } else { base }
    } else {
        let base = value.ceil();
        let frac = value - base;
        if frac < -0.5 { base - 1.0 } else { base }
    }
}

/// Apply scale and offset conversion to a raw float register value.
pub fn apply_scale(node: &FloatNode, raw: f64) -> f64 {
    let mut value = raw;
    if let Some((num, den)) = node.scale {
        value *= num as f64 / den as f64;
    }
    if let Some(offset) = node.offset {
        value += offset;
    }
    value
}

/// Encode a user-facing float value back to raw register representation.
pub fn encode_float(node: &FloatNode, value: f64) -> Result<i64, GenApiError> {
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

/// Map a bitops error to a GenApiError with node context.
pub fn map_bitops_error(name: &str, err: BitOpsError) -> GenApiError {
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

/// Get raw bytes from cache or read from device for read-modify-write operations.
///
/// This helper is used when writing to a bitfield requires first reading the current
/// register value, modifying specific bits, and writing back the result.
pub fn get_raw_or_read(
    cache: &std::cell::RefCell<Option<Vec<u8>>>,
    io: &dyn crate::RegisterIo,
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
