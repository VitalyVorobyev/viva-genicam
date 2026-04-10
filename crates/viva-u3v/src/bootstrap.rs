//! USB3 Vision bootstrap register maps.
//!
//! The device exposes a hierarchy of register maps that describe its
//! capabilities and provide addresses for higher-level structures:
//!
//! ```text
//! ABRM (addr 0x0000, GenCP standard)
//!   ├─ manufacturer / model / serial strings
//!   ├─ ManifestTable address → GenICam XML location
//!   └─ SBRM address → technology-specific registers
//!         ├─ max transfer sizes
//!         ├─ SIRM address → streaming config
//!         └─ EIRM address → event config
//! ```

use bytes::Buf;

use crate::control::ControlChannel;
use crate::usb::UsbTransfer;
use crate::U3vError;

// ---------------------------------------------------------------------------
// ABRM — Application Bootstrap Register Map (GenCP standard, at addr 0x0000)
// ---------------------------------------------------------------------------

/// ABRM register offsets (GenCP §5.3).
mod abrm_reg {
    pub const GENCP_VERSION: u64 = 0x0000;
    pub const MANUFACTURER_NAME: u64 = 0x0048;
    pub const MODEL_NAME: u64 = 0x0088;
    pub const FAMILY_NAME: u64 = 0x00C8;
    pub const DEVICE_VERSION: u64 = 0x0108;
    pub const SERIAL_NUMBER: u64 = 0x01A8;
    pub const USER_DEFINED_NAME: u64 = 0x01E8;
    pub const MANIFEST_TABLE_ADDR: u64 = 0x0228;
    pub const SBRM_ADDRESS: u64 = 0x0230;
    pub const DEVICE_CAPABILITY: u64 = 0x0238;

    /// Maximum length of a string register (64 bytes including NUL).
    pub const STRING_LEN: usize = 64;
}

/// Application Bootstrap Register Map — GenCP standard registers at address 0.
#[derive(Debug, Clone)]
pub struct Abrm {
    pub gencp_version: u32,
    pub manufacturer_name: String,
    pub model_name: String,
    pub family_name: String,
    pub device_version: String,
    pub serial_number: String,
    pub user_defined_name: String,
    pub manifest_table_address: u64,
    pub sbrm_address: u64,
    pub device_capability: u64,
}

impl Abrm {
    /// Read the ABRM from the device.
    pub fn read_from<T: UsbTransfer>(control: &mut ControlChannel<T>) -> Result<Self, U3vError> {
        let read_string = |ctrl: &mut ControlChannel<T>, addr: u64| -> Result<String, U3vError> {
            let raw = ctrl.read_mem(addr, abrm_reg::STRING_LEN)?;
            Ok(parse_nul_string(&raw))
        };

        let gencp_ver_bytes = control.read_mem(abrm_reg::GENCP_VERSION, 4)?;
        let gencp_version = read_u32_be(&gencp_ver_bytes);

        let manufacturer_name = read_string(control, abrm_reg::MANUFACTURER_NAME)?;
        let model_name = read_string(control, abrm_reg::MODEL_NAME)?;
        let family_name = read_string(control, abrm_reg::FAMILY_NAME)?;
        let device_version = read_string(control, abrm_reg::DEVICE_VERSION)?;
        let serial_number = read_string(control, abrm_reg::SERIAL_NUMBER)?;
        let user_defined_name = read_string(control, abrm_reg::USER_DEFINED_NAME)?;

        let manifest_bytes = control.read_mem(abrm_reg::MANIFEST_TABLE_ADDR, 8)?;
        let manifest_table_address = read_u64_be(&manifest_bytes);

        let sbrm_bytes = control.read_mem(abrm_reg::SBRM_ADDRESS, 8)?;
        let sbrm_address = read_u64_be(&sbrm_bytes);

        let cap_bytes = control.read_mem(abrm_reg::DEVICE_CAPABILITY, 8)?;
        let device_capability = read_u64_be(&cap_bytes);

        Ok(Self {
            gencp_version,
            manufacturer_name,
            model_name,
            family_name,
            device_version,
            serial_number,
            user_defined_name,
            manifest_table_address,
            sbrm_address,
            device_capability,
        })
    }
}

// ---------------------------------------------------------------------------
// SBRM — Serial (technology-specific) Bootstrap Register Map
// ---------------------------------------------------------------------------

/// SBRM register offsets relative to the SBRM base address.
mod sbrm_reg {
    pub const U3V_VERSION: u64 = 0x0000;
    pub const MAX_CMD_TRANSFER: u64 = 0x0004;
    pub const MAX_ACK_TRANSFER: u64 = 0x0008;
    pub const NUM_STREAM_CHANNELS: u64 = 0x000C;
    pub const SIRM_ADDRESS: u64 = 0x0010;
    pub const SIRM_LENGTH: u64 = 0x0018;
    pub const EIRM_ADDRESS: u64 = 0x001C;
    pub const EIRM_LENGTH: u64 = 0x0024;
}

/// Technology-specific Bootstrap Register Map for USB3 Vision.
#[derive(Debug, Clone)]
pub struct Sbrm {
    pub base: u64,
    pub u3v_version: u32,
    pub max_cmd_transfer: u32,
    pub max_ack_transfer: u32,
    pub num_stream_channels: u32,
    pub sirm_address: u64,
    pub sirm_length: u32,
    pub eirm_address: u64,
    pub eirm_length: u32,
}

impl Sbrm {
    /// Read the SBRM from the device at the given base address (from ABRM).
    pub fn read_from<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        base: u64,
    ) -> Result<Self, U3vError> {
        let read_u32 = |ctrl: &mut ControlChannel<T>, off: u64| -> Result<u32, U3vError> {
            let bytes = ctrl.read_mem(base + off, 4)?;
            Ok(read_u32_be(&bytes))
        };
        let read_u64 = |ctrl: &mut ControlChannel<T>, off: u64| -> Result<u64, U3vError> {
            let bytes = ctrl.read_mem(base + off, 8)?;
            Ok(read_u64_be(&bytes))
        };

        Ok(Self {
            base,
            u3v_version: read_u32(control, sbrm_reg::U3V_VERSION)?,
            max_cmd_transfer: read_u32(control, sbrm_reg::MAX_CMD_TRANSFER)?,
            max_ack_transfer: read_u32(control, sbrm_reg::MAX_ACK_TRANSFER)?,
            num_stream_channels: read_u32(control, sbrm_reg::NUM_STREAM_CHANNELS)?,
            sirm_address: read_u64(control, sbrm_reg::SIRM_ADDRESS)?,
            sirm_length: read_u32(control, sbrm_reg::SIRM_LENGTH)?,
            eirm_address: read_u64(control, sbrm_reg::EIRM_ADDRESS)?,
            eirm_length: read_u32(control, sbrm_reg::EIRM_LENGTH)?,
        })
    }
}

// ---------------------------------------------------------------------------
// SIRM — Streaming Interface Register Map
// ---------------------------------------------------------------------------

/// SIRM register offsets relative to the SIRM base address.
mod sirm_reg {
    pub const SIRM_INFO: u64 = 0x0000;
    pub const SIRM_CONTROL: u64 = 0x0004;
    pub const REQ_PAYLOAD_SIZE: u64 = 0x0008;
    pub const REQ_LEADER_SIZE: u64 = 0x0010;
    pub const REQ_TRAILER_SIZE: u64 = 0x0014;
    pub const MAX_LEADER_SIZE: u64 = 0x0018;
    pub const MAX_TRAILER_SIZE: u64 = 0x001C;
    pub const PAYLOAD_SIZE: u64 = 0x0020;
    pub const PAYLOAD_COUNT: u64 = 0x0028;
    pub const TRANSFER1_SIZE: u64 = 0x002C;
    pub const TRANSFER2_SIZE: u64 = 0x0030;
    pub const MAX_PAYLOAD_TRANSFER: u64 = 0x0034;
}

/// Streaming Interface Register Map — per-stream-channel configuration.
#[derive(Debug, Clone)]
pub struct Sirm {
    pub base: u64,
    pub info: u32,
    pub control: u32,
    pub req_payload_size: u64,
    pub req_leader_size: u32,
    pub req_trailer_size: u32,
    pub max_leader_size: u32,
    pub max_trailer_size: u32,
    pub payload_size: u64,
    pub payload_count: u32,
    pub transfer1_size: u32,
    pub transfer2_size: u32,
    pub max_payload_transfer: u32,
}

impl Sirm {
    /// Read the SIRM from the device at the given base address (from SBRM).
    pub fn read_from<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        base: u64,
    ) -> Result<Self, U3vError> {
        let read_u32 = |ctrl: &mut ControlChannel<T>, off: u64| -> Result<u32, U3vError> {
            let bytes = ctrl.read_mem(base + off, 4)?;
            Ok(read_u32_be(&bytes))
        };
        let read_u64 = |ctrl: &mut ControlChannel<T>, off: u64| -> Result<u64, U3vError> {
            let bytes = ctrl.read_mem(base + off, 8)?;
            Ok(read_u64_be(&bytes))
        };

        Ok(Self {
            base,
            info: read_u32(control, sirm_reg::SIRM_INFO)?,
            control: read_u32(control, sirm_reg::SIRM_CONTROL)?,
            req_payload_size: read_u64(control, sirm_reg::REQ_PAYLOAD_SIZE)?,
            req_leader_size: read_u32(control, sirm_reg::REQ_LEADER_SIZE)?,
            req_trailer_size: read_u32(control, sirm_reg::REQ_TRAILER_SIZE)?,
            max_leader_size: read_u32(control, sirm_reg::MAX_LEADER_SIZE)?,
            max_trailer_size: read_u32(control, sirm_reg::MAX_TRAILER_SIZE)?,
            payload_size: read_u64(control, sirm_reg::PAYLOAD_SIZE)?,
            payload_count: read_u32(control, sirm_reg::PAYLOAD_COUNT)?,
            transfer1_size: read_u32(control, sirm_reg::TRANSFER1_SIZE)?,
            transfer2_size: read_u32(control, sirm_reg::TRANSFER2_SIZE)?,
            max_payload_transfer: read_u32(control, sirm_reg::MAX_PAYLOAD_TRANSFER)?,
        })
    }
}

impl Sirm {
    /// Enable streaming by setting bit 0 of the SIRM control register.
    pub fn enable<T: UsbTransfer>(&self, control: &mut ControlChannel<T>) -> Result<(), U3vError> {
        let val = self.control | 0x0000_0001;
        control.write_mem(self.base + sirm_reg::SIRM_CONTROL, &val.to_be_bytes())
    }

    /// Disable streaming by clearing bit 0 of the SIRM control register.
    pub fn disable<T: UsbTransfer>(&self, control: &mut ControlChannel<T>) -> Result<(), U3vError> {
        let val = self.control & !0x0000_0001;
        control.write_mem(self.base + sirm_reg::SIRM_CONTROL, &val.to_be_bytes())
    }

    /// Write the requested payload, leader, and trailer sizes to the SIRM.
    pub fn configure<T: UsbTransfer>(
        &self,
        control: &mut ControlChannel<T>,
        payload_size: u64,
        leader_size: u32,
        trailer_size: u32,
    ) -> Result<(), U3vError> {
        control.write_mem(
            self.base + sirm_reg::REQ_PAYLOAD_SIZE,
            &payload_size.to_be_bytes(),
        )?;
        control.write_mem(
            self.base + sirm_reg::REQ_LEADER_SIZE,
            &leader_size.to_be_bytes(),
        )?;
        control.write_mem(
            self.base + sirm_reg::REQ_TRAILER_SIZE,
            &trailer_size.to_be_bytes(),
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Manifest table entry (for locating GenICam XML)
// ---------------------------------------------------------------------------

/// A single entry in the ABRM ManifestTable.
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    pub file_address: u64,
    pub file_size: u64,
}

impl ManifestEntry {
    /// Read the first manifest entry from the manifest table at the given
    /// address (from ABRM). Only the first entry is read; multi-entry
    /// tables are uncommon in practice.
    pub fn read_first<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        table_address: u64,
    ) -> Result<Self, U3vError> {
        // Manifest table header: entry count (u32) at offset 0, then entries.
        // Each entry: 8-byte file info + 8-byte address + 8-byte size.
        let header = control.read_mem(table_address, 4)?;
        let count = read_u32_be(&header);
        if count == 0 {
            return Err(U3vError::Protocol("manifest table is empty".into()));
        }

        // First entry starts at offset 8 (after 4-byte count + 4 reserved).
        let entry_offset = table_address + 8;
        let entry_data = control.read_mem(entry_offset, 24)?;
        // Entry layout: [8 bytes file info] [8 bytes address] [8 bytes size]
        let file_address = read_u64_be(&entry_data[8..16]);
        let file_size = read_u64_be(&entry_data[16..24]);

        Ok(Self {
            file_address,
            file_size,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u32_be(data: &[u8]) -> u32 {
    let mut buf = &data[..4];
    buf.get_u32()
}

fn read_u64_be(data: &[u8]) -> u64 {
    let mut buf = &data[..8];
    buf.get_u64()
}

fn parse_nul_string(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;
    use bytes::{BufMut, BytesMut};
    use std::sync::Arc;

    const EP_OUT: u8 = 0x01;
    const EP_IN: u8 = 0x81;
    const ACK_PREFIX_LE: u32 = 0x4356_3341;
    const PREFIX_SIZE: usize = 12;

    /// Build a successful ack carrying `payload`.
    fn success_ack(request_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(PREFIX_SIZE + payload.len());
        buf.put_u32_le(ACK_PREFIX_LE);
        buf.put_u16_le(0x0000); // Success
        buf.put_u16_le(0x0085); // ReadMem ack opcode
        buf.put_u16_le(payload.len() as u16);
        buf.put_u16_le(request_id);
        buf.extend_from_slice(payload);
        buf.to_vec()
    }

    /// Helper: enqueue a sequence of ack responses for sequential read_mem calls.
    /// Returns the next request_id after all enqueued acks.
    fn enqueue_read_responses(
        mock: &MockUsbTransfer,
        ep_in: u8,
        start_req_id: u16,
        payloads: &[Vec<u8>],
    ) -> u16 {
        let mut req_id = start_req_id;
        for payload in payloads {
            mock.enqueue_read(ep_in, success_ack(req_id, payload));
            req_id = req_id.wrapping_add(1);
        }
        req_id
    }

    #[test]
    fn parse_nul_terminated_string() {
        let mut data = vec![0u8; 64];
        let s = b"TestCamera";
        data[..s.len()].copy_from_slice(s);
        assert_eq!(parse_nul_string(&data), "TestCamera");
    }

    #[test]
    fn parse_string_no_nul() {
        let data = b"FullBuffer64Chars";
        assert_eq!(parse_nul_string(data), "FullBuffer64Chars");
    }

    #[test]
    fn sbrm_read_from_mock() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 1024, 1024);

        let sbrm_base: u64 = 0x1_0000;
        let mut req_id: u16 = 0;

        // SBRM fields in read order: u3v_version, max_cmd, max_ack,
        // num_streams, sirm_addr, sirm_len, eirm_addr, eirm_len
        let payloads: Vec<Vec<u8>> = vec![
            0x0001_0000u32.to_be_bytes().to_vec(), // u3v_version
            1024u32.to_be_bytes().to_vec(),        // max_cmd_transfer
            1024u32.to_be_bytes().to_vec(),        // max_ack_transfer
            1u32.to_be_bytes().to_vec(),           // num_stream_channels
            0x0002_0000u64.to_be_bytes().to_vec(), // sirm_address
            256u32.to_be_bytes().to_vec(),         // sirm_length
            0x0003_0000u64.to_be_bytes().to_vec(), // eirm_address
            64u32.to_be_bytes().to_vec(),          // eirm_length
        ];
        req_id = enqueue_read_responses(&mock, EP_IN, req_id, &payloads);
        let _ = req_id;

        let sbrm = Sbrm::read_from(&mut ch, sbrm_base).unwrap();
        assert_eq!(sbrm.u3v_version, 0x0001_0000);
        assert_eq!(sbrm.max_cmd_transfer, 1024);
        assert_eq!(sbrm.max_ack_transfer, 1024);
        assert_eq!(sbrm.num_stream_channels, 1);
        assert_eq!(sbrm.sirm_address, 0x0002_0000);
        assert_eq!(sbrm.sirm_length, 256);
        assert_eq!(sbrm.eirm_address, 0x0003_0000);
        assert_eq!(sbrm.eirm_length, 64);
    }

    #[test]
    fn manifest_entry_read_first() {
        let mock = Arc::new(MockUsbTransfer::new());
        let mut ch = ControlChannel::new(Arc::clone(&mock), EP_IN, EP_OUT, 1024, 1024);

        let table_addr: u64 = 0x5000;
        // Header: count = 1
        mock.enqueue_read(EP_IN, success_ack(0, &1u32.to_be_bytes()));
        // Entry: [8 bytes info][8 bytes address][8 bytes size]
        let mut entry = BytesMut::with_capacity(24);
        entry.put_u64(0); // file info (version, schema, etc.)
        entry.put_u64(0x0010_0000); // file_address
        entry.put_u64(4096); // file_size
        mock.enqueue_read(EP_IN, success_ack(1, &entry));

        let manifest = ManifestEntry::read_first(&mut ch, table_addr).unwrap();
        assert_eq!(manifest.file_address, 0x0010_0000);
        assert_eq!(manifest.file_size, 4096);
    }
}
