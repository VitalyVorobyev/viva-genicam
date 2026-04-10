use thiserror::Error;
use tracing::debug;
use viva_genapi_xml::{BitField, ByteOrder};

/// Errors produced by bitfield extraction and insertion helpers.
#[derive(Debug, Error)]
pub enum BitOpsError {
    /// Register payload wider than the supported 64-bit limit.
    #[error("unsupported register width {len} bytes for bitfield operation")]
    UnsupportedWidth { len: usize },
    /// Bitfield length outside the supported range.
    #[error("unsupported bitfield length {bit_length} bits")]
    UnsupportedLength { bit_length: u16 },
    /// Bitfield configuration exceeds the provided register payload.
    #[error(
        "bitfield (offset {bit_offset}, length {bit_length}) exceeds register width {len} bytes"
    )]
    OutOfRange {
        len: usize,
        bit_offset: u16,
        bit_length: u16,
    },
    /// Provided value does not fit into the bitfield.
    #[error("value {value} does not fit {bit_length} bits")]
    ValueTooWide { bit_length: u16, value: u64 },
}

fn validate_range(bits: &[u8], bf: BitField) -> Result<(), BitOpsError> {
    if bits.is_empty() || bits.len() > 8 {
        return Err(BitOpsError::UnsupportedWidth { len: bits.len() });
    }
    if bf.bit_length == 0 || bf.bit_length > 64 {
        return Err(BitOpsError::UnsupportedLength {
            bit_length: bf.bit_length,
        });
    }
    let total_bits = bits.len() * 8;
    let start = bf.bit_offset as usize;
    let length = bf.bit_length as usize;
    if start + length > total_bits {
        return Err(BitOpsError::OutOfRange {
            len: bits.len(),
            bit_offset: bf.bit_offset,
            bit_length: bf.bit_length,
        });
    }
    Ok(())
}

fn mask_for(length: u16) -> u128 {
    if length == 64 {
        u64::MAX as u128
    } else {
        (1u128 << length) - 1
    }
}

/// Extract the value of a bitfield from the provided register payload.
pub fn extract(bits: &[u8], bf: BitField) -> Result<u64, BitOpsError> {
    validate_range(bits, bf)?;
    let mask = mask_for(bf.bit_length);
    let mut value = 0u128;
    match bf.byte_order {
        ByteOrder::Little => {
            for (index, byte) in bits.iter().enumerate() {
                value |= (*byte as u128) << (index * 8);
            }
            value = (value >> bf.bit_offset as u32) & mask;
        }
        ByteOrder::Big => {
            for byte in bits {
                value = (value << 8) | (*byte as u128);
            }
            let total_bits = (bits.len() * 8) as u32;
            let shift = total_bits - bf.bit_offset as u32 - bf.bit_length as u32;
            value = (value >> shift) & mask;
        }
    }
    let result = value as u64;
    debug!(
        bit_offset = bf.bit_offset,
        bit_length = bf.bit_length,
        order = ?bf.byte_order,
        bytes = ?bits,
        value = result,
        "extract bitfield"
    );
    Ok(result)
}

/// Insert a value into the specified bitfield within the destination payload.
pub fn insert(dst: &mut [u8], bf: BitField, value: u64) -> Result<(), BitOpsError> {
    validate_range(dst, bf)?;
    let mask = mask_for(bf.bit_length);
    if (value as u128) > mask {
        return Err(BitOpsError::ValueTooWide {
            bit_length: bf.bit_length,
            value,
        });
    }
    let mut full = 0u128;
    match bf.byte_order {
        ByteOrder::Little => {
            for (index, byte) in dst.iter().enumerate() {
                full |= (*byte as u128) << (index * 8);
            }
            let clear_mask = !(mask << bf.bit_offset as u32);
            full = (full & clear_mask) | ((value as u128) << bf.bit_offset as u32);
            for (index, byte) in dst.iter_mut().enumerate() {
                *byte = ((full >> (index * 8)) & 0xFF) as u8;
            }
        }
        ByteOrder::Big => {
            let len = dst.len();
            for byte in dst.iter() {
                full = (full << 8) | (*byte as u128);
            }
            let total_bits = (len * 8) as u32;
            let shift = total_bits - bf.bit_offset as u32 - bf.bit_length as u32;
            let clear_mask = !(mask << shift);
            full = (full & clear_mask) | ((value as u128) << shift);
            for (index, byte) in dst.iter_mut().enumerate() {
                let shift = ((len - 1 - index) * 8) as u32;
                *byte = ((full >> shift) & 0xFF) as u8;
            }
        }
    }
    debug!(
        bit_offset = bf.bit_offset,
        bit_length = bf.bit_length,
        order = ?bf.byte_order,
        bytes = ?dst,
        value,
        "insert bitfield"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(bit_offset: u16, bit_length: u16, byte_order: ByteOrder) -> BitField {
        BitField {
            bit_offset,
            bit_length,
            byte_order,
        }
    }

    #[test]
    fn extract_little_endian_across_bytes() {
        let data = [0x12, 0x34, 0x56, 0x78];
        let bf = field(8, 8, ByteOrder::Little);
        let value = extract(&data, bf).expect("extract");
        assert_eq!(value, 0x34);
    }

    #[test]
    fn extract_big_endian_high_bits() {
        let data = [0b1010_0000, 0b1111_0000];
        let bf = field(0, 3, ByteOrder::Big);
        let value = extract(&data, bf).expect("extract");
        assert_eq!(value, 0b101);
    }

    #[test]
    fn insert_roundtrip_little_endian() {
        let mut data = [0u8; 4];
        let bf = field(8, 8, ByteOrder::Little);
        insert(&mut data, bf, 0xAB).expect("insert");
        assert_eq!(data, [0x00, 0xAB, 0x00, 0x00]);
        let value = extract(&data, bf).expect("extract");
        assert_eq!(value, 0xAB);
    }

    #[test]
    fn insert_roundtrip_big_endian() {
        let mut data = [0x80, 0x00];
        let bf = field(0, 3, ByteOrder::Big);
        insert(&mut data, bf, 0b010).expect("insert");
        assert_eq!(data, [0b0100_0000, 0x00]);
        let value = extract(&data, bf).expect("extract");
        assert_eq!(value, 0b010);
    }

    #[test]
    fn insert_rejects_large_value() {
        let mut data = [0u8; 2];
        let bf = field(4, 4, ByteOrder::Little);
        let err = insert(&mut data, bf, 0x20).unwrap_err();
        assert!(matches!(err, BitOpsError::ValueTooWide { .. }));
    }
}
