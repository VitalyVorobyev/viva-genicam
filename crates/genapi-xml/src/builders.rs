//! Builder types for constructing addressing and bitfield metadata.

use tracing::warn;

use crate::{Addressing, BitField, ByteOrder, XmlError};

/// Builder for collecting address-related elements during parsing.
#[derive(Debug, Default)]
pub struct AddressingBuilder {
    pub(crate) fixed_address: Option<u64>,
    pub(crate) length: Option<u32>,
    pub(crate) selector: Option<String>,
    pub(crate) entries: Vec<AddressEntry>,
    pub(crate) pending_value: Option<String>,
    pub(crate) pending_len: Option<u32>,
    pub(crate) p_address_node: Option<String>,
}

/// A single selector-to-address mapping entry.
#[derive(Debug, Clone)]
pub struct AddressEntry {
    pub value: String,
    pub address: u64,
    pub len: Option<u32>,
}

impl AddressingBuilder {
    /// Set a fixed register address.
    pub fn set_fixed_address(&mut self, address: u64) {
        self.fixed_address = Some(address);
    }

    /// Set the register length in bytes.
    pub fn set_length(&mut self, len: u32) {
        self.length = Some(len);
    }

    /// Set an indirect address provider node.
    pub fn set_p_address_node(&mut self, node: &str) {
        self.p_address_node = Some(node.to_string());
    }

    /// Register a selector node for address switching.
    pub fn register_selector(&mut self, selector: &str) {
        if self.selector.is_none() {
            self.selector = Some(selector.to_string());
        }
    }

    /// Push a selector value for the next address attachment.
    pub fn push_selected_value(&mut self, value: String) {
        self.pending_value = Some(value);
        self.pending_len = None;
    }

    /// Apply a length value, either to the pending selector entry or globally.
    pub fn apply_length(&mut self, len: u32) {
        if self.pending_value.is_some() {
            self.pending_len = Some(len);
        } else {
            self.length = Some(len);
        }
    }

    /// Attach an address to the current pending selector value.
    pub fn attach_selected_address(&mut self, address: u64, len_override: Option<u32>) {
        if let Some(value) = self.pending_value.take() {
            let len = len_override.or(self.pending_len.take());
            self.entries.push(AddressEntry {
                value,
                address,
                len,
            });
        } else {
            self.fixed_address = Some(address);
            if let Some(len) = len_override {
                self.length = Some(len);
            }
        }
    }

    /// Finalize the builder into an [`Addressing`] variant.
    pub fn finalize(self, node: &str, default_len: Option<u32>) -> Result<Addressing, XmlError> {
        if !self.entries.is_empty() {
            let selector = self.selector.ok_or_else(|| {
                XmlError::Invalid(format!(
                    "node {node} provides <Selected> addresses without <pSelected>"
                ))
            })?;
            let mut map = Vec::new();
            for entry in self.entries {
                let len = entry.len.or(self.length).or(default_len).ok_or_else(|| {
                    XmlError::Invalid(format!(
                        "node {node} is missing <Length> for selector value {}",
                        entry.value
                    ))
                })?;
                if let Some(existing) = map.iter_mut().find(|(value, _)| *value == entry.value) {
                    *existing = (entry.value.clone(), (entry.address, len));
                } else {
                    map.push((entry.value.clone(), (entry.address, len)));
                }
            }
            if self.p_address_node.is_some() {
                warn!(
                    node = %node,
                    "ignoring <pAddress> in favour of selector table"
                );
            }
            if self.fixed_address.is_some() {
                warn!(
                    node = %node,
                    selector = %selector,
                    "ignoring fixed <Address> in favour of selector table"
                );
            }
            Ok(Addressing::BySelector { selector, map })
        } else {
            let len = self
                .length
                .or(default_len)
                .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Length>")))?;
            if let Some(p_address_node) = self.p_address_node {
                if self.fixed_address.is_some() {
                    warn!(
                        node = %node,
                        address_node = %p_address_node,
                        "ignoring fixed <Address> in favour of <pAddress>"
                    );
                }
                Ok(Addressing::Indirect {
                    p_address_node,
                    len,
                })
            } else {
                let address = self.fixed_address.ok_or_else(|| {
                    XmlError::Invalid(format!("node {node} is missing <Address>"))
                })?;
                Ok(Addressing::Fixed { address, len })
            }
        }
    }
}

/// Source of bitfield specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitfieldSource {
    /// LSB/MSB pair defining the range.
    LsbMsb,
    /// Bit index and optional length.
    BitLength,
    /// Bitmask value.
    Mask,
}

/// Builder for collecting bitfield-related elements during parsing.
#[derive(Debug, Default)]
pub struct BitfieldBuilder {
    lsb: Option<u32>,
    msb: Option<u32>,
    bit: Option<u32>,
    bit_length: Option<u32>,
    mask: Option<u64>,
    byte_order: Option<ByteOrder>,
    source: Option<BitfieldSource>,
}

impl BitfieldBuilder {
    /// Record an LSB value.
    pub fn note_lsb(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::LsbMsb)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::LsbMsb);
        self.lsb = Some(value);
    }

    /// Record an MSB value.
    pub fn note_msb(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::LsbMsb)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::LsbMsb);
        self.msb = Some(value);
    }

    /// Record a bit index.
    pub fn note_bit(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::BitLength)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::BitLength);
        self.bit = Some(value);
    }

    /// Record a bit length.
    pub fn note_bit_length(&mut self, value: u32) {
        if self
            .source
            .map(|source| source != BitfieldSource::BitLength)
            .unwrap_or(false)
        {
            return;
        }
        self.source.get_or_insert(BitfieldSource::BitLength);
        self.bit_length = Some(value);
    }

    /// Record a bitmask.
    pub fn note_mask(&mut self, mask: u64) {
        if self.source.is_some() {
            return;
        }
        self.source = Some(BitfieldSource::Mask);
        self.mask = Some(mask);
    }

    /// Record a byte order.
    pub fn note_byte_order(&mut self, order: ByteOrder) {
        self.byte_order = Some(order);
    }

    /// Finalize the builder into a [`BitField`] if sufficient data was provided.
    pub fn finish(self, node: &str, lengths: &[u32]) -> Result<Option<BitField>, XmlError> {
        let source = match self.source {
            Some(source) => source,
            None => return Ok(None),
        };
        let byte_order = self.byte_order.unwrap_or(ByteOrder::Little);
        if lengths.is_empty() {
            return Err(XmlError::Invalid(format!(
                "node {node} is missing register length information"
            )));
        }
        let mut unique_len = None;
        for len in lengths {
            if *len == 0 {
                return Err(XmlError::Invalid(format!(
                    "node {node} declares zero-length register"
                )));
            }
            if let Some(existing) = unique_len {
                if existing != *len {
                    return Err(XmlError::Invalid(format!(
                        "node {node} uses inconsistent register lengths"
                    )));
                }
            } else {
                unique_len = Some(*len);
            }
        }
        let len_bytes = unique_len.unwrap_or(0);
        let total_bits = len_bytes
            .checked_mul(8)
            .ok_or_else(|| XmlError::Invalid(format!("node {node} register length overflow")))?;

        let (offset_lsb, bit_length) = match source {
            BitfieldSource::LsbMsb => {
                let lsb = self
                    .lsb
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Lsb>")))?;
                let msb = self
                    .msb
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Msb>")))?;
                let lower = lsb.min(msb);
                let upper = lsb.max(msb);
                let length = upper
                    .checked_sub(lower)
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| {
                        XmlError::Invalid(format!(
                            "node {node} has invalid bit range <Lsb>={lsb}, <Msb>={msb}"
                        ))
                    })?;
                (lower, length)
            }
            BitfieldSource::BitLength => {
                let bit = self
                    .bit
                    .ok_or_else(|| XmlError::Invalid(format!("node {node} is missing <Bit>")))?;
                let length = self.bit_length.unwrap_or(1);
                (bit, length)
            }
            BitfieldSource::Mask => {
                let mask = self.mask.ok_or_else(|| {
                    XmlError::Invalid(format!("node {node} is missing <Mask> value"))
                })?;
                if mask == 0 {
                    return Err(XmlError::Invalid(format!(
                        "node {node} mask must be non-zero"
                    )));
                }
                let offset = mask.trailing_zeros();
                let length = mask.count_ones();
                (offset, length)
            }
        };

        if bit_length == 0 {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield must have positive length"
            )));
        }
        if bit_length > 64 {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield length {bit_length} exceeds 64 bits"
            )));
        }

        if offset_lsb > u16::MAX as u32 {
            return Err(XmlError::Invalid(format!(
                "node {node} bit offset {offset_lsb} exceeds u16 range"
            )));
        }

        if bit_length > u16::MAX as u32 {
            return Err(XmlError::Invalid(format!(
                "node {node} bit length {bit_length} exceeds u16 range"
            )));
        }

        if offset_lsb + bit_length > total_bits {
            return Err(XmlError::Invalid(format!(
                "node {node} bitfield exceeds register width"
            )));
        }

        let offset = match byte_order {
            ByteOrder::Little => offset_lsb,
            ByteOrder::Big => total_bits - bit_length - offset_lsb,
        };

        Ok(Some(BitField {
            bit_offset: u16::try_from(offset).map_err(|_| {
                XmlError::Invalid(format!("node {node} bit offset {offset} exceeds u16 range"))
            })?,
            bit_length: u16::try_from(bit_length).map_err(|_| {
                XmlError::Invalid(format!(
                    "node {node} bit length {bit_length} exceeds u16 range"
                ))
            })?,
            byte_order,
        }))
    }
}

/// Extract lengths from an addressing variant.
pub fn addressing_lengths(addressing: &Addressing) -> Vec<u32> {
    match addressing {
        Addressing::Fixed { len, .. } => vec![*len],
        Addressing::Indirect { len, .. } => vec![*len],
        Addressing::BySelector { map, .. } => map.iter().map(|(_, (_, len))| *len).collect(),
    }
}
