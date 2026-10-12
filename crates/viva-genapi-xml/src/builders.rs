//! Builder types for constructing addressing and bitfield metadata.

use crate::{AddressTerm, Addressing, BitField, ByteOrder, IndexOffset, XmlError};

/// Builder for collecting address-related elements during parsing.
///
/// GenICam register addresses are additive: `<Address>`, `<pAddress>` and
/// `<pIndex>` may each appear several times and every one of them contributes.
/// This builder therefore *appends* terms; it never replaces one with another.
#[derive(Debug, Default)]
pub(crate) struct AddressingBuilder {
    pub(crate) terms: Vec<AddressTerm>,
    pub(crate) length: Option<u32>,
    pub(crate) port: Option<String>,
}

impl AddressingBuilder {
    /// Finalize the builder into an [`Addressing`] using default length 0.
    ///
    /// This is useful for StringReg nodes where the length is optional and can
    /// default to the full register.
    pub(crate) fn build(self) -> Addressing {
        let len = self.length.unwrap_or(0);
        let terms = if self.terms.is_empty() {
            vec![AddressTerm::Fixed(0)]
        } else {
            self.terms
        };
        Addressing::Sum {
            terms,
            len,
            port: self.port,
        }
    }

    /// Add a literal `<Address>` term.
    pub(crate) fn push_fixed_address(&mut self, address: i64) {
        self.terms.push(AddressTerm::Fixed(address));
    }

    /// Record the `<pPort>` the address is relative to.
    pub(crate) fn set_port(&mut self, port: &str) {
        self.port = Some(port.to_string());
    }

    /// Set the register length in bytes.
    pub(crate) fn set_length(&mut self, len: u32) {
        self.length = Some(len);
    }

    /// Add a `<pAddress>` term naming a node that supplies an offset.
    pub(crate) fn push_p_address(&mut self, node: &str) {
        self.terms.push(AddressTerm::Node(node.to_string()));
    }

    /// Add a `<pIndex>` term: an index node scaled by a stride.
    pub(crate) fn push_index(&mut self, node: &str, offset: IndexOffset) {
        self.terms.push(AddressTerm::Index {
            node: node.to_string(),
            offset,
        });
    }

    /// Finalize the builder into an [`Addressing`].
    pub(crate) fn finalize(
        self,
        node: &str,
        default_len: Option<u32>,
    ) -> Result<Addressing, XmlError> {
        let len = self
            .length
            .or(default_len)
            .ok_or_else(|| XmlError::missing(node, "<Length>"))?;
        if self.terms.is_empty() {
            return Err(XmlError::missing(node, "<Address>"));
        }
        Ok(Addressing::Sum {
            terms: self.terms,
            len,
            port: self.port,
        })
    }
}

/// Source of bitfield specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitfieldSource {
    /// LSB/MSB pair defining the range.
    LsbMsb,
    /// Bit index and optional length.
    BitLength,
    /// Bitmask value.
    Mask,
}

/// Builder for collecting bitfield-related elements during parsing.
#[derive(Debug, Default)]
pub(crate) struct BitfieldBuilder {
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
    pub(crate) fn note_lsb(&mut self, value: u32) {
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
    pub(crate) fn note_msb(&mut self, value: u32) {
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
    pub(crate) fn note_bit(&mut self, value: u32) {
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
    pub(crate) fn note_bit_length(&mut self, value: u32) {
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
    pub(crate) fn note_mask(&mut self, mask: u64) {
        if self.source.is_some() {
            return;
        }
        self.source = Some(BitfieldSource::Mask);
        self.mask = Some(mask);
    }

    /// Record a byte order.
    pub(crate) fn note_byte_order(&mut self, order: ByteOrder) {
        self.byte_order = Some(order);
    }

    /// Finalize the builder into a [`BitField`] if sufficient data was provided.
    pub(crate) fn finish(self, node: &str, len_bytes: u32) -> Result<Option<BitField>, XmlError> {
        let source = match self.source {
            Some(source) => source,
            None => return Ok(None),
        };
        let byte_order = self.byte_order.unwrap_or(ByteOrder::Little);
        if len_bytes == 0 {
            return Err(XmlError::invalid(node, "declares a zero-length register"));
        }
        let total_bits = len_bytes
            .checked_mul(8)
            .ok_or_else(|| XmlError::invalid(node, "declares a register too long to address"))?;

        // Whether the index produced below is already counted the way
        // `bitops` counts for this byte order — see the `offset` match at the
        // end of this function for what that means and why it differs by
        // source.
        let index_matches_byte_order = !matches!(source, BitfieldSource::Mask);

        let (first_index, bit_length) = match source {
            BitfieldSource::LsbMsb => {
                let lsb = self.lsb.ok_or_else(|| XmlError::missing(node, "<LSB>"))?;
                let msb = self.msb.ok_or_else(|| XmlError::missing(node, "<MSB>"))?;
                // GenICam orients the pair by endianness: on `Big` the
                // indices count down from the MSB, so a conformant document
                // has `<LSB>` >= `<MSB>`; on `Little` they count up, so
                // `<LSB>` <= `<MSB>`. Across the 38-document vendor corpus the
                // split is absolute — 1307 big-endian declarations with
                // `LSB > MSB` and none the other way, 41 little-endian with
                // `LSB < MSB` and none the other way. Taking min/max reads the
                // right field under either orientation, but a document that
                // contradicts its own byte order is the one signal that a
                // vendor used the other convention, so say so rather than
                // normalising it away in silence (ADR-0018).
                if lsb != msb {
                    let inverted = match byte_order {
                        ByteOrder::Big => lsb < msb,
                        ByteOrder::Little => lsb > msb,
                    };
                    if inverted {
                        tracing::debug!(
                            node,
                            lsb,
                            msb,
                            order = ?byte_order,
                            "bit range is oriented against its byte order; reading it as \
                             min..=max"
                        );
                    }
                }
                let lower = lsb.min(msb);
                let upper = lsb.max(msb);
                let length = upper
                    .checked_sub(lower)
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| XmlError::invalid(node, "has an invalid bit range"))?;
                (lower, length)
            }
            BitfieldSource::BitLength => {
                let bit = self.bit.ok_or_else(|| XmlError::missing(node, "<Bit>"))?;
                let length = self.bit_length.unwrap_or(1);
                (bit, length)
            }
            BitfieldSource::Mask => {
                let mask = self.mask.ok_or_else(|| XmlError::missing(node, "<Mask>"))?;
                if mask == 0 {
                    return Err(XmlError::invalid(node, "has a zero <Mask>"));
                }
                let offset = mask.trailing_zeros();
                let length = mask.count_ones();
                (offset, length)
            }
        };

        if bit_length == 0 {
            return Err(XmlError::invalid(node, "declares an empty bit field"));
        }
        if bit_length > 64 {
            return Err(XmlError::invalid(
                node,
                "declares a bit field wider than 64 bits",
            ));
        }

        if first_index > u16::MAX as u32 {
            return Err(XmlError::invalid(
                node,
                "declares a bit offset past bit 65535",
            ));
        }

        if bit_length > u16::MAX as u32 {
            return Err(XmlError::invalid(
                node,
                "declares a bit field wider than 65535 bits",
            ));
        }

        if first_index + bit_length > total_bits {
            return Err(XmlError::invalid(
                node,
                "declares a bit field wider than its register",
            ));
        }

        // [`BitField::bit_offset`] is relative to the *most* significant bit for
        // `Big` and to the least significant bit for `Little`, matching
        // `viva_genapi::bitops`.
        //
        // GenICam numbers `<LSB>`, `<MSB>` and `<Bit>` the same way: from the
        // MSB on a big-endian register, from the LSB otherwise. So for those
        // sources the XML index is already in the target frame and must be used
        // as-is. Converting it — as this function did until issue #120 — flips
        // it a second time in `bitops` and cancels out, which read big-endian
        // registers off the wrong end. FLIR's `ExposureTime_Imp` (`<Bit>0</Bit>`,
        // big-endian, 4 bytes) resolved to bit 0 instead of bit 31, so every
        // write to `ExposureTime` was refused locally as unavailable.
        //
        // `<Mask>` is the exception, and the reason this match is keyed on the
        // source rather than only on the byte order: a mask is a literal
        // register value, so `trailing_zeros` is inherently LSB-relative and
        // still has to be converted for `Big`.
        let offset = match (byte_order, index_matches_byte_order) {
            (_, true) => first_index,
            (ByteOrder::Little, false) => first_index,
            (ByteOrder::Big, false) => total_bits - bit_length - first_index,
        };

        Ok(Some(BitField {
            bit_offset: u16::try_from(offset)
                .map_err(|_| XmlError::invalid(node, "declares a bit offset past bit 65535"))?,
            bit_length: u16::try_from(bit_length).map_err(|_| {
                XmlError::invalid(node, "declares a bit field wider than 65535 bits")
            })?,
            byte_order,
        }))
    }
}
