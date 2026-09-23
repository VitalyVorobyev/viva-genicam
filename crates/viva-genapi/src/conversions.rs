//! Numeric conversion utilities for register values and bitfields.

use tracing::debug;
use viva_genapi_xml::{ByteOrder, Sign};

use crate::GenApiError;
use crate::bitops::BitOpsError;
use crate::nodes::FloatNode;

/// Convert a register payload (up to 8 bytes) to a 64-bit integer.
///
/// `order` is the payload's byte order as declared by `<Endianess>` /
/// `<Endianness>` / `<ByteOrder>`; GenICam defaults to [`ByteOrder::Big`].
///
/// `sign` decides whether the payload's top bit means "negative" or is just
/// another value bit. GenICam defaults to [`Sign::Unsigned`], and getting it
/// wrong is silent right up until a register's top bit is set: a
/// `GevCurrentIPAddress` of `192.168.1.160` (`0xC0A801A0`) then reads as
/// `-1062731360`.
///
/// A full-width unsigned payload whose top bit is set does not fit `i64`, and
/// GenApi has nowhere else to put it — `IInteger` *is* `int64` (GenICam
/// v2.1.1 §2.4). It is reinterpreted as two's complement rather than refused,
/// which is lossless at the bit level and round-trips through
/// [`i64_to_bytes`]. See ADR-0022.
pub fn bytes_to_i64(
    name: &str,
    bytes: &[u8],
    sign: Sign,
    order: ByteOrder,
) -> Result<i64, GenApiError> {
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
    // Normalise to big-endian first so sign extension has a single form.
    let mut be = [0u8; 8];
    let offset = 8 - bytes.len();
    match order {
        ByteOrder::Big => be[offset..].copy_from_slice(bytes),
        ByteOrder::Little => {
            for (i, byte) in bytes.iter().rev().enumerate() {
                be[offset + i] = *byte;
            }
        }
    }
    if sign.is_signed() && (be[offset] & 0x80) != 0 {
        for byte in &mut be[..offset] {
            *byte = 0xFF;
        }
    }
    let value = i64::from_be_bytes(be);
    if !sign.is_signed() && bytes.len() == 8 && value < 0 {
        // Reinterpreted, not refused — see the note on this function. Logged
        // because the value a caller sees is negative and the register is not.
        debug!(
            node = %name,
            "unsigned 64-bit register reinterpreted as two's-complement i64"
        );
    }
    Ok(value)
}

/// Convert a 64-bit integer to a register payload of the given width and order.
///
/// The inverse of [`bytes_to_i64`], and validated against it: a value that does
/// not survive the round trip does not fit the register. Keeping the two in
/// step matters more than either alone — a reader that learns byte order while
/// the writer does not turns every little-endian write into a range error.
pub fn i64_to_bytes(
    name: &str,
    value: i64,
    width: u32,
    sign: Sign,
    order: ByteOrder,
) -> Result<Vec<u8>, GenApiError> {
    if width == 0 || width > 8 {
        return Err(GenApiError::Parse(format!(
            "node {name} has unsupported width {width}"
        )));
    }
    let width = width as usize;
    let bytes = value.to_be_bytes();
    let mut data = bytes[8 - width..].to_vec();
    if matches!(order, ByteOrder::Little) {
        data.reverse();
    }
    let roundtrip = bytes_to_i64(name, &data, sign, order)?;
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

/// Decode an IEEE 754 payload into an `f64`.
///
/// `bytes.len()` must be 4 (f32) or 8 (f64). Other widths are rejected because
/// the GenICam Float schema has no other native encodings.
pub fn decode_ieee754(name: &str, bytes: &[u8], order: ByteOrder) -> Result<f64, GenApiError> {
    match bytes.len() {
        4 => {
            let b: [u8; 4] = bytes.try_into().expect("len checked");
            let v = match order {
                ByteOrder::Big => f32::from_be_bytes(b),
                ByteOrder::Little => f32::from_le_bytes(b),
            };
            Ok(v as f64)
        }
        8 => {
            let b: [u8; 8] = bytes.try_into().expect("len checked");
            Ok(match order {
                ByteOrder::Big => f64::from_be_bytes(b),
                ByteOrder::Little => f64::from_le_bytes(b),
            })
        }
        other => Err(GenApiError::Parse(format!(
            "node {name} unsupported IEEE-754 width {other}"
        ))),
    }
}

/// Encode an `f64` as IEEE 754 bytes for a register of width `len` (4 or 8).
///
/// Rejects non-finite inputs and f32 overflow so the caller gets a typed
/// `GenApiError::Range` rather than a silent inf write.
pub fn encode_ieee754(
    name: &str,
    value: f64,
    len: u32,
    order: ByteOrder,
) -> Result<Vec<u8>, GenApiError> {
    match len {
        4 => {
            if !value.is_finite() {
                return Err(GenApiError::Range(name.to_string()));
            }
            if value < f32::MIN as f64 || value > f32::MAX as f64 {
                return Err(GenApiError::Range(name.to_string()));
            }
            let v = value as f32;
            Ok(match order {
                ByteOrder::Big => v.to_be_bytes().to_vec(),
                ByteOrder::Little => v.to_le_bytes().to_vec(),
            })
        }
        8 => {
            if !value.is_finite() {
                return Err(GenApiError::Range(name.to_string()));
            }
            Ok(match order {
                ByteOrder::Big => value.to_be_bytes().to_vec(),
                ByteOrder::Little => value.to_le_bytes().to_vec(),
            })
        }
        other => Err(GenApiError::Parse(format!(
            "node {name} unsupported IEEE-754 width {other}"
        ))),
    }
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
///
/// `readable` is whether the node's access mode permits a read. A write-only
/// register with a warm cache is still writable — the cached bytes are what
/// this node last wrote, which is the legal GenICam way to keep the other bits
/// — but with a cold cache the read would go to a device that refuses it, so
/// it is refused here instead with an error that says why (GA-31).
pub fn get_raw_or_read(
    name: &str,
    readable: bool,
    cache: &std::cell::RefCell<Option<Vec<u8>>>,
    io: &dyn crate::RegisterIo,
    address: u64,
    len: u32,
) -> Result<Vec<u8>, GenApiError> {
    let cached = cache.borrow().clone();
    if let Some(bytes) = cached
        && bytes.len() == len as usize
    {
        return Ok(bytes);
    }
    if !readable {
        return Err(GenApiError::MaskedWriteUnreadable {
            name: name.to_string(),
            address,
        });
    }
    io.read(address, len as usize).map_err(|err| match err {
        GenApiError::Io(_) => err,
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// Counts reads and answers every one, so a test can assert that none was
    /// issued rather than that one failed.
    struct CountingIo {
        reads: Cell<usize>,
    }

    impl crate::RegisterIo for CountingIo {
        fn read(&self, _addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            self.reads.set(self.reads.get() + 1);
            Ok(vec![0; len])
        }

        fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
            Ok(())
        }
    }

    /// GA-31: a cold cache on an unreadable register is refused without a
    /// read; a warm one is used; a readable register still reads.
    #[test]
    fn read_modify_write_never_reads_a_write_only_register() {
        let io = CountingIo {
            reads: Cell::new(0),
        };

        let cold = RefCell::new(None);
        let err = get_raw_or_read("Pulse", false, &cold, &io, 0x200B0, 4)
            .expect_err("a cold write-only register cannot supply its other bits");
        assert!(
            matches!(
                err,
                GenApiError::MaskedWriteUnreadable { ref name, address: 0x200B0 } if name == "Pulse"
            ),
            "got {err:?}"
        );
        assert_eq!(io.reads.get(), 0, "no read may reach the transport");

        let warm = RefCell::new(Some(vec![0x80, 0, 0, 0]));
        let raw = get_raw_or_read("Pulse", false, &warm, &io, 0x200B0, 4).expect("warm cache");
        assert_eq!(raw, vec![0x80, 0, 0, 0]);
        assert_eq!(io.reads.get(), 0, "a warm cache needs no read");

        get_raw_or_read("Flag", true, &cold, &io, 0x100, 4).expect("readable register");
        assert_eq!(io.reads.get(), 1, "a readable register is read as before");
    }

    /// Issue #140 and #112: a `GevTimestampValue`-shaped register whose top bit
    /// is set. Refusing it made the node unreadable on FLIR and Vieworks
    /// hardware; the bits are handed back reinterpreted instead.
    #[test]
    fn full_width_unsigned_is_reinterpreted_not_refused() {
        let all_ones = [0xFFu8; 8];
        assert_eq!(
            bytes_to_i64("Ts", &all_ones, Sign::Unsigned, ByteOrder::Big).unwrap(),
            -1
        );

        // The exact value quoted in #140.
        let bytes = 0x8000_0000_0000_002Au64.to_be_bytes();
        assert_eq!(
            bytes_to_i64("Ts", &bytes, Sign::Unsigned, ByteOrder::Big).unwrap(),
            i64::MIN + 42
        );
    }

    /// The write half of the same guard: this failed before the fix, which is
    /// the read-modify-write case on #112's timestamps.
    #[test]
    fn full_width_unsigned_round_trips_through_the_encoder() {
        let encoded = i64_to_bytes("Ts", -1, 8, Sign::Unsigned, ByteOrder::Big).unwrap();
        assert_eq!(encoded, vec![0xFF; 8]);
        assert_eq!(
            bytes_to_i64("Ts", &encoded, Sign::Unsigned, ByteOrder::Big).unwrap(),
            -1
        );
    }

    /// GA-28: 311 plain `<IntReg>` declarations across 16 of the 38 corpus
    /// documents declare `LittleEndian`, and every one of them decoded
    /// byte-swapped.
    #[test]
    fn little_endian_payloads_decode_and_encode_swapped() {
        for (width, value) in [
            (1u32, 0x12i64),
            (2, 0x1234),
            (4, 0x1234_5678),
            (8, 0x1234_5678_9ABC_DEF0),
        ] {
            let be = i64_to_bytes("R", value, width, Sign::Unsigned, ByteOrder::Big).unwrap();
            let le = i64_to_bytes("R", value, width, Sign::Unsigned, ByteOrder::Little).unwrap();
            let mut reversed = be.clone();
            reversed.reverse();
            assert_eq!(le, reversed, "width {width}");

            assert_eq!(
                bytes_to_i64("R", &be, Sign::Unsigned, ByteOrder::Big).unwrap(),
                value,
                "width {width} big"
            );
            assert_eq!(
                bytes_to_i64("R", &le, Sign::Unsigned, ByteOrder::Little).unwrap(),
                value,
                "width {width} little"
            );
        }
    }

    /// Sign extension has to happen after the payload is normalised, not
    /// before: the sign bit of a little-endian value lives in the last byte.
    #[test]
    fn sign_extension_follows_byte_order() {
        // -2 as a 2-byte register: 0xFFFE big-endian, 0xFEFF little-endian.
        assert_eq!(
            bytes_to_i64("R", &[0xFF, 0xFE], Sign::Signed, ByteOrder::Big).unwrap(),
            -2
        );
        assert_eq!(
            bytes_to_i64("R", &[0xFE, 0xFF], Sign::Signed, ByteOrder::Little).unwrap(),
            -2
        );
        // The same bytes read unsigned stay positive.
        assert_eq!(
            bytes_to_i64("R", &[0xFF, 0xFE], Sign::Unsigned, ByteOrder::Big).unwrap(),
            0xFFFE
        );
        assert_eq!(
            bytes_to_i64("R", &[0xFE, 0xFF], Sign::Unsigned, ByteOrder::Little).unwrap(),
            0xFFFE
        );
    }

    /// A value too wide for the register must still be refused — the round-trip
    /// check is what catches it, and it now runs in both byte orders.
    #[test]
    fn out_of_range_values_are_refused_in_both_orders() {
        for order in [ByteOrder::Big, ByteOrder::Little] {
            assert!(matches!(
                i64_to_bytes("R", 0x1_0000, 2, Sign::Unsigned, order),
                Err(GenApiError::Range(_))
            ));
            assert!(matches!(
                i64_to_bytes("R", -1, 2, Sign::Unsigned, order),
                Err(GenApiError::Range(_))
            ));
        }
    }

    #[test]
    fn empty_and_oversized_payloads_are_rejected() {
        assert!(bytes_to_i64("R", &[], Sign::Unsigned, ByteOrder::Big).is_err());
        assert!(bytes_to_i64("R", &[0; 9], Sign::Unsigned, ByteOrder::Big).is_err());
        assert!(i64_to_bytes("R", 0, 0, Sign::Unsigned, ByteOrder::Big).is_err());
        assert!(i64_to_bytes("R", 0, 9, Sign::Unsigned, ByteOrder::Big).is_err());
    }
}
