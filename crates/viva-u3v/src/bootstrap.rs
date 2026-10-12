//! USB3 Vision bootstrap register maps.
//!
//! The device exposes a hierarchy of register maps that describe its
//! capabilities and provide addresses for higher-level structures:
//!
//! ```text
//! ABRM (addr 0x0000, GenCP standard)
//!   ├─ manufacturer / model / serial strings
//!   ├─ Manifest Table address → GenICam XML location
//!   └─ SBRM address → USB3 Vision registers
//!         ├─ max transfer sizes
//!         ├─ SIRM address → streaming config
//!         └─ EIRM address → event config
//! ```
//!
//! Every bootstrap register is **little-endian**: GenCP requires the
//! bootstrap registers to share the protocol's byte order (GenCP 1.2 §2.3),
//! which for USB3 Vision is little-endian (see [`crate::control`]). Where the
//! layouts come from, strongest evidence first:
//!
//! - **ABRM**: GenCP 1.2, Table 15, with the capability bits of Table 17 and
//!   the manifest of Tables 29-30.
//! - **SBRM**: the USB3 Vision standard, which is not freely available. A
//!   Grasshopper3's own XML maps U3VCP capability (+0x04), max command and
//!   acknowledge transfer (+0x14, +0x18) and stream channel count (+0x1C),
//!   all `LittleEndian` (`PGR_GS3_U3_23S6M.xml`); aravis (`arvuvcpprivate.h`)
//!   and Wireshark (`packet-u3v.c`) agree on those and add the SIRM and EIRM
//!   addresses.
//! - **SIRM**: aravis and Wireshark, which agree with each other.
//!
//! Until backlog TC-27 this module read every register big-endian at offsets
//! of its own; no USB3 Vision camera had been reported against it.

use crate::U3vError;
use crate::control::ControlChannel;
use crate::stream::StreamLayout;
use crate::usb::UsbTransfer;

// ---------------------------------------------------------------------------
// ABRM — Technology Agnostic Bootstrap Register Map (GenCP standard, at 0)
// ---------------------------------------------------------------------------

/// ABRM register offsets (GenCP 1.2, Table 15).
pub mod abrm {
    /// `u32`: major version in the upper 16 bits, minor in the lower.
    pub const GENCP_VERSION: u64 = 0x0000;
    /// 64-byte string.
    pub const MANUFACTURER_NAME: u64 = 0x0004;
    /// 64-byte string.
    pub const MODEL_NAME: u64 = 0x0044;
    /// 64-byte string; present when [`capability::FAMILY_NAME`](super::capability::FAMILY_NAME) is set.
    pub const FAMILY_NAME: u64 = 0x0084;
    /// 64-byte string.
    pub const DEVICE_VERSION: u64 = 0x00C4;
    /// 64-byte string.
    pub const MANUFACTURER_INFO: u64 = 0x0104;
    /// 64-byte string.
    pub const SERIAL_NUMBER: u64 = 0x0144;
    /// 64-byte string; present when [`capability::USER_DEFINED_NAME`](super::capability::USER_DEFINED_NAME) is set.
    pub const USER_DEFINED_NAME: u64 = 0x0184;
    /// `u64` bit field, [`capability`](super::capability).
    pub const DEVICE_CAPABILITY: u64 = 0x01C4;
    /// `u32`, milliseconds.
    pub const MAX_DEVICE_RESPONSE_TIME: u64 = 0x01CC;
    /// `u64`.
    pub const MANIFEST_TABLE_ADDRESS: u64 = 0x01D0;
    /// `u64`; present when [`capability::SBRM`](super::capability::SBRM) is set.
    pub const SBRM_ADDRESS: u64 = 0x01D8;
    /// `u64` bit field.
    pub const DEVICE_CONFIGURATION: u64 = 0x01E0;
    /// `u32`, milliseconds.
    pub const HEARTBEAT_TIMEOUT: u64 = 0x01E8;
    /// `u32`: the id of the device's message channel (GenCP 1.2 Table 15).
    pub const MESSAGE_CHANNEL_ID: u64 = 0x01EC;
    /// `u32`.
    pub const ACCESS_PRIVILEGE: u64 = 0x0204;
    /// `u32`: 0 big-endian, `0xFFFF_FFFF` little-endian, for the device's own
    /// registers; present when [`capability::ENDIANNESS_REGISTER`](super::capability::ENDIANNESS_REGISTER) is set.
    pub const IMPLEMENTATION_ENDIANNESS: u64 = 0x020C;
    /// Bytes from the start of the ABRM to the reserved space.
    pub const LEN: u64 = 0x0210;
    /// Length of every string register.
    pub const STRING_LEN: usize = 64;
}

/// Device Capability bits (GenCP 1.2, Table 17).
pub mod capability {
    /// The User Defined Name register exists.
    pub const USER_DEFINED_NAME: u64 = 1 << 0;
    /// Heartbeat and Access Privilege are supported.
    pub const ACCESS_PRIVILEGE: u64 = 1 << 1;
    /// The device has a message channel.
    pub const MESSAGE_CHANNEL: u64 = 1 << 2;
    /// The device has a timestamp register.
    pub const TIMESTAMP: u64 = 1 << 3;
    /// Bits 4-7: the bootstrap strings' encoding, 0 ASCII, 1 UTF-8, 2 UTF-16.
    pub const STRING_ENCODING_SHIFT: u32 = 4;
    /// The Family Name register exists.
    pub const FAMILY_NAME: u64 = 1 << 8;
    /// The device has an SBRM.
    pub const SBRM: u64 = 1 << 9;
    /// The Implementation Endianness register exists.
    pub const ENDIANNESS_REGISTER: u64 = 1 << 10;
    /// A WRITEMEM_ACK says how many bytes were written.
    pub const WRITTEN_LENGTH: u64 = 1 << 11;
}

/// Technology Agnostic Bootstrap Register Map — GenCP's registers at address 0.
#[derive(Debug, Clone)]
pub struct Abrm {
    /// Major version in the upper 16 bits, minor in the lower.
    pub gencp_version: u32,
    /// Manufacturer name.
    pub manufacturer_name: String,
    /// Model name.
    pub model_name: String,
    /// Empty when the device has no Family Name register.
    pub family_name: String,
    /// Device version.
    pub device_version: String,
    /// Manufacturer-specific information.
    pub manufacturer_info: String,
    /// Serial number.
    pub serial_number: String,
    /// Empty when the device has no User Defined Name register.
    pub user_defined_name: String,
    /// [`capability`] bits.
    pub device_capability: u64,
    /// How long the device may take to execute a command, not counting the
    /// transfers, in milliseconds.
    pub max_device_response_time_ms: u32,
    /// Where the manifest table is.
    pub manifest_table_address: u64,
    /// Where the SBRM is.
    pub sbrm_address: u64,
}

impl Abrm {
    /// Read the ABRM from the device.
    ///
    /// The GenCP version is read first, as GenCP recommends for the first
    /// command of a session (§3.1); conditional registers are read only when
    /// the Device Capability register says they exist, since a device answers
    /// a missing one with `GENCP_INVALID_ADDRESS` (§5.3).
    pub fn read_from<T: UsbTransfer>(control: &mut ControlChannel<T>) -> Result<Self, U3vError> {
        let gencp_version = read_u32(control, abrm::GENCP_VERSION)?;
        let device_capability = read_u64(control, abrm::DEVICE_CAPABILITY)?;
        let encoding = StringEncoding::from_capability(device_capability);
        let read_string =
            |control: &mut ControlChannel<T>, addr: u64| -> Result<String, U3vError> {
                let raw = control.read_mem(addr, abrm::STRING_LEN)?;
                Ok(encoding.decode(&raw))
            };
        let read_optional_string =
            |control: &mut ControlChannel<T>, bit: u64, addr: u64| -> Result<String, U3vError> {
                if device_capability & bit == 0 {
                    return Ok(String::new());
                }
                read_string(control, addr)
            };

        Ok(Self {
            gencp_version,
            manufacturer_name: read_string(control, abrm::MANUFACTURER_NAME)?,
            model_name: read_string(control, abrm::MODEL_NAME)?,
            family_name: read_optional_string(control, capability::FAMILY_NAME, abrm::FAMILY_NAME)?,
            device_version: read_string(control, abrm::DEVICE_VERSION)?,
            manufacturer_info: read_string(control, abrm::MANUFACTURER_INFO)?,
            serial_number: read_string(control, abrm::SERIAL_NUMBER)?,
            user_defined_name: read_optional_string(
                control,
                capability::USER_DEFINED_NAME,
                abrm::USER_DEFINED_NAME,
            )?,
            device_capability,
            max_device_response_time_ms: read_u32(control, abrm::MAX_DEVICE_RESPONSE_TIME)?,
            manifest_table_address: read_u64(control, abrm::MANIFEST_TABLE_ADDRESS)?,
            // Conditional in GenCP, but a USB3 Vision device without one has
            // no stream and no transfer limits; aravis reads it regardless.
            sbrm_address: read_u64(control, abrm::SBRM_ADDRESS)?,
        })
    }
}

/// How the bootstrap strings are encoded (Device Capability bits 4-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringEncoding {
    Ascii,
    Utf8,
    Utf16,
}

impl StringEncoding {
    fn from_capability(capability: u64) -> Self {
        match (capability >> capability::STRING_ENCODING_SHIFT) & 0xF {
            1 => Self::Utf8,
            2 => Self::Utf16,
            // 0 is ASCII; reserved values are read as the most common case.
            _ => Self::Ascii,
        }
    }

    /// Decode a string register: unused bytes are 0, and a full register may
    /// omit the terminator (GenCP 1.2 §5.2). UTF-16 code units follow the
    /// bootstrap byte order, little-endian.
    fn decode(self, raw: &[u8]) -> String {
        match self {
            Self::Ascii | Self::Utf8 => parse_nul_string(raw),
            Self::Utf16 => {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&pair| u16::from_le_bytes(pair))
                    .take_while(|&unit| unit != 0)
                    .collect();
                String::from_utf16_lossy(&units)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SBRM — USB3 Vision (technology-specific) Bootstrap Register Map
// ---------------------------------------------------------------------------

/// SBRM register offsets from the SBRM base address.
pub mod sbrm {
    /// `u32`: major version in the upper 16 bits, minor in the lower.
    pub const U3V_VERSION: u64 = 0x0000;
    /// `u64` bit field, [`u3vcp_capability`](super::u3vcp_capability).
    pub const U3VCP_CAPABILITY: u64 = 0x0004;
    /// `u64` bit field.
    pub const U3VCP_CONFIGURATION: u64 = 0x000C;
    /// `u32`, bytes.
    pub const MAX_CMD_TRANSFER: u64 = 0x0014;
    /// `u32`, bytes.
    pub const MAX_ACK_TRANSFER: u64 = 0x0018;
    /// `u32`.
    pub const NUM_STREAM_CHANNELS: u64 = 0x001C;
    /// `u64`.
    pub const SIRM_ADDRESS: u64 = 0x0020;
    /// `u32`, bytes.
    pub const SIRM_LENGTH: u64 = 0x0028;
    /// `u64`.
    pub const EIRM_ADDRESS: u64 = 0x002C;
    /// `u32`, bytes.
    pub const EIRM_LENGTH: u64 = 0x0034;
    /// `u64`.
    pub const IIDC2_ADDRESS: u64 = 0x0038;
    /// `u32`: the bus speed the device runs at.
    pub const CURRENT_SPEED: u64 = 0x0040;
    /// Bytes from the SBRM base to its reserved space.
    pub const LEN: u64 = 0x0044;
}

/// U3VCP Capability bits, as the Grasshopper3 XML maps them
/// (`U3VCPSIRMAvailable` bit 0, `U3VCPEIRMAvailable` bit 1,
/// `U3VCPIIDC2Available` bit 2).
pub mod u3vcp_capability {
    /// The device has a SIRM.
    pub const SIRM_AVAILABLE: u64 = 1 << 0;
    /// The device has an EIRM.
    pub const EIRM_AVAILABLE: u64 = 1 << 1;
    /// The device has an IIDC2 register map.
    pub const IIDC2_AVAILABLE: u64 = 1 << 2;
}

/// USB3 Vision's technology-specific Bootstrap Register Map.
#[derive(Debug, Clone)]
pub struct Sbrm {
    /// Where the SBRM is.
    pub base: u64,
    /// Major version in the upper 16 bits, minor in the lower.
    pub u3v_version: u32,
    /// [`u3vcp_capability`] bits.
    pub u3vcp_capability: u64,
    /// Largest command transfer the device accepts, in bytes.
    pub max_cmd_transfer: u32,
    /// Largest acknowledge transfer it sends, in bytes.
    pub max_ack_transfer: u32,
    /// Number of stream channels.
    pub num_stream_channels: u32,
    /// 0 when the device has no SIRM.
    pub sirm_address: u64,
    /// Length of the SIRM in bytes.
    pub sirm_length: u32,
    /// 0 when the device has no EIRM.
    pub eirm_address: u64,
    /// Length of the EIRM in bytes.
    pub eirm_length: u32,
}

impl Sbrm {
    /// Read the SBRM from the device at the given base address (from ABRM).
    ///
    /// The SIRM and EIRM registers are read whatever the capability bits
    /// say, as aravis does, and a refusal reads as "absent": a device whose
    /// bit and register disagree still streams.
    pub fn read_from<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        base: u64,
    ) -> Result<Self, U3vError> {
        let u3vcp_capability = read_u64(control, base + sbrm::U3VCP_CAPABILITY)?;
        let optional_u64 = |control: &mut ControlChannel<T>, offset: u64| {
            or_absent(read_u64(control, base + offset))
        };
        let optional_u32 = |control: &mut ControlChannel<T>, offset: u64| {
            or_absent(read_u32(control, base + offset))
        };
        Ok(Self {
            base,
            u3v_version: read_u32(control, base + sbrm::U3V_VERSION)?,
            u3vcp_capability,
            max_cmd_transfer: read_u32(control, base + sbrm::MAX_CMD_TRANSFER)?,
            max_ack_transfer: read_u32(control, base + sbrm::MAX_ACK_TRANSFER)?,
            num_stream_channels: read_u32(control, base + sbrm::NUM_STREAM_CHANNELS)?,
            sirm_address: optional_u64(control, sbrm::SIRM_ADDRESS)?,
            sirm_length: optional_u32(control, sbrm::SIRM_LENGTH)?,
            eirm_address: optional_u64(control, sbrm::EIRM_ADDRESS)?,
            eirm_length: optional_u32(control, sbrm::EIRM_LENGTH)?,
        })
    }
}

/// A register the device refuses to read is absent, read as 0; a transport
/// failure is still a failure.
fn or_absent<V: Default>(read: Result<V, U3vError>) -> Result<V, U3vError> {
    match read {
        Err(U3vError::Status(status)) => {
            tracing::debug!(
                %status,
                "optional bootstrap register refused; reading as absent"
            );
            Ok(V::default())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// SIRM — Streaming Interface Register Map
// ---------------------------------------------------------------------------

/// SIRM register offsets from the SIRM base address.
///
/// The device fills in Info and the three *required* sizes; the host writes
/// the transfer layout and Control.
pub mod sirm {
    /// `u32`: bits 24-31 are the log2 of the transfer alignment.
    pub const INFO: u64 = 0x0000;
    /// `u32`: bit 0 is StreamEnable.
    pub const CONTROL: u64 = 0x0004;
    /// `u64`, read-only: bytes of payload in one block.
    pub const REQUIRED_PAYLOAD_SIZE: u64 = 0x0008;
    /// `u32`, read-only: bytes the leader needs.
    pub const REQUIRED_LEADER_SIZE: u64 = 0x0010;
    /// `u32`, read-only: bytes the trailer needs.
    pub const REQUIRED_TRAILER_SIZE: u64 = 0x0014;
    /// `u32`, written by the host: the leader transfer's size.
    pub const MAX_LEADER_SIZE: u64 = 0x0018;
    /// `u32`, written by the host: each payload transfer's size.
    pub const PAYLOAD_TRANSFER_SIZE: u64 = 0x001C;
    /// `u32`, written by the host: how many payload transfers of that size.
    pub const PAYLOAD_TRANSFER_COUNT: u64 = 0x0020;
    /// `u32`, written by the host: the first final transfer's size.
    pub const FINAL_TRANSFER1_SIZE: u64 = 0x0024;
    /// `u32`, written by the host: the second final transfer's size.
    pub const FINAL_TRANSFER2_SIZE: u64 = 0x0028;
    /// `u32`, written by the host: the trailer transfer's size.
    pub const MAX_TRAILER_SIZE: u64 = 0x002C;
    /// Bytes in the SIRM.
    pub const LEN: u64 = 0x0030;
    /// Control's StreamEnable bit.
    pub const STREAM_ENABLE: u32 = 1 << 0;
}

/// What the device says about one stream channel.
#[derive(Debug, Clone)]
pub struct Sirm {
    /// Where the SIRM is.
    pub base: u64,
    /// `SI Info`: the transfer alignment, as a power of two in bits 24-31.
    pub info: u32,
    /// `SI Control`: bit 0 enables streaming.
    pub control: u32,
    /// `Required Payload Size`: bytes a block's payload takes.
    pub required_payload_size: u64,
    /// `Required Leader Size`.
    pub required_leader_size: u32,
    /// `Required Trailer Size`.
    pub required_trailer_size: u32,
}

impl Sirm {
    /// Read the device's half of the SIRM at the given base address (from
    /// SBRM).
    pub fn read_from<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        base: u64,
    ) -> Result<Self, U3vError> {
        Ok(Self {
            base,
            info: read_u32(control, base + sirm::INFO)?,
            control: read_u32(control, base + sirm::CONTROL)?,
            required_payload_size: read_u64(control, base + sirm::REQUIRED_PAYLOAD_SIZE)?,
            required_leader_size: read_u32(control, base + sirm::REQUIRED_LEADER_SIZE)?,
            required_trailer_size: read_u32(control, base + sirm::REQUIRED_TRAILER_SIZE)?,
        })
    }

    /// The alignment every transfer size must keep, from Info bits 24-31.
    pub fn alignment(&self) -> Result<u32, U3vError> {
        let exponent = self.info >> 24;
        1u32.checked_shl(exponent)
            .ok_or_else(|| U3vError::Device("asks for a transfer alignment beyond 2^31 bytes"))
    }

    /// Write the transfer layout the host will read with. The required sizes
    /// are the device's and are never written: they are read-only, and a
    /// device refuses the write (Codex review of #181).
    pub fn configure<T: UsbTransfer>(
        &self,
        control: &mut ControlChannel<T>,
        layout: &StreamLayout,
    ) -> Result<(), U3vError> {
        for (offset, value) in [
            (sirm::MAX_LEADER_SIZE, layout.leader_size),
            (sirm::MAX_TRAILER_SIZE, layout.trailer_size),
            (sirm::PAYLOAD_TRANSFER_SIZE, layout.payload_transfer_size),
            (sirm::PAYLOAD_TRANSFER_COUNT, layout.payload_transfer_count),
            (sirm::FINAL_TRANSFER1_SIZE, layout.final_transfer1_size),
            (sirm::FINAL_TRANSFER2_SIZE, layout.final_transfer2_size),
        ] {
            control.write_mem(self.base + offset, &value.to_le_bytes())?;
        }
        Ok(())
    }

    /// Set StreamEnable.
    pub fn enable<T: UsbTransfer>(&self, control: &mut ControlChannel<T>) -> Result<(), U3vError> {
        control.write_mem(
            self.base + sirm::CONTROL,
            &sirm::STREAM_ENABLE.to_le_bytes(),
        )
    }

    /// Clear StreamEnable.
    pub fn disable<T: UsbTransfer>(&self, control: &mut ControlChannel<T>) -> Result<(), U3vError> {
        control.write_mem(self.base + sirm::CONTROL, &0u32.to_le_bytes())
    }
}

// ---------------------------------------------------------------------------
// Manifest (for locating GenICam XML)
// ---------------------------------------------------------------------------

/// Manifest layout (GenCP 1.2, Tables 29-30).
mod manifest {
    /// The table starts with a `u64` entry count.
    pub(super) const ENTRIES: u64 = 8;
    /// Each entry is 64 bytes.
    pub(super) const ENTRY_LEN: u64 = 64;
    /// Entries looked at before giving up on finding a device XML.
    pub(super) const MAX_ENTRIES: u64 = 16;
}

/// How a manifest file is stored (Schema / Filetype / Fileformat bits 10-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileFormat {
    /// An uncompressed GenICam XML file.
    Uncompressed,
    /// A ZIP archive holding a single GenICam XML file.
    Zip,
    /// A value GenCP 1.2 reserves.
    Reserved(u8),
}

/// One entry of the device's Manifest Table: a GenICam file in its memory.
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// Where the file is.
    pub file_address: u64,
    /// Its size in bytes.
    pub file_size: u64,
    /// How it is stored.
    pub format: FileFormat,
}

impl ManifestEntry {
    /// The first entry describing a device XML (file type 0) in the
    /// Manifest Table at `table_address` (from ABRM). A buffer XML (file
    /// type 1) describes chunk data only and is skipped.
    pub fn read_device_xml<T: UsbTransfer>(
        control: &mut ControlChannel<T>,
        table_address: u64,
    ) -> Result<Self, U3vError> {
        let count = read_u64(control, table_address)?;
        if count == 0 {
            return Err(U3vError::Device("has an empty manifest table"));
        }
        for index in 0..count.min(manifest::MAX_ENTRIES) {
            let entry_address = table_address + manifest::ENTRIES + index * manifest::ENTRY_LEN;
            let entry = control.read_mem(entry_address, 24)?;
            let schema = u32::from_le_bytes(entry[4..8].try_into().unwrap());
            let file_type = schema & 0x7;
            if file_type != 0 {
                continue;
            }
            let format = match ((schema >> 10) & 0x3F) as u8 {
                0 => FileFormat::Uncompressed,
                1 => FileFormat::Zip,
                other => FileFormat::Reserved(other),
            };
            return Ok(Self {
                file_address: u64::from_le_bytes(entry[8..16].try_into().unwrap()),
                file_size: u64::from_le_bytes(entry[16..24].try_into().unwrap()),
                format,
            });
        }
        Err(U3vError::Device("lists no device XML in its manifest"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u32<T: UsbTransfer>(control: &mut ControlChannel<T>, addr: u64) -> Result<u32, U3vError> {
    let bytes = control.read_mem(addr, 4)?;
    Ok(u32::from_le_bytes(bytes[..4].try_into().unwrap()))
}

fn read_u64<T: UsbTransfer>(control: &mut ControlChannel<T>, addr: u64) -> Result<u64, U3vError> {
    let bytes = control.read_mem(addr, 8)?;
    Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
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

    /// Device Capability bits 4-7 name the strings' encoding; UTF-16 code
    /// units are little-endian, as every bootstrap field is.
    #[test]
    fn strings_follow_the_capability_encoding() {
        let utf16 = StringEncoding::from_capability(2 << capability::STRING_ENCODING_SHIFT);
        assert_eq!(utf16, StringEncoding::Utf16);
        let mut raw = [0u8; 64];
        for (i, unit) in "Ünï".encode_utf16().enumerate() {
            raw[2 * i..2 * i + 2].copy_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(utf16.decode(&raw), "Ünï");

        let utf8 = StringEncoding::from_capability(1 << capability::STRING_ENCODING_SHIFT);
        assert_eq!(utf8.decode("Ünï\0".as_bytes()), "Ünï");
        assert_eq!(StringEncoding::from_capability(0), StringEncoding::Ascii);
    }

    #[test]
    fn alignment_comes_from_info_bits_24_to_31() {
        let sirm = |info| Sirm {
            base: 0,
            info,
            control: 0,
            required_payload_size: 0,
            required_leader_size: 0,
            required_trailer_size: 0,
        };
        assert_eq!(sirm(0).alignment().unwrap(), 1);
        assert_eq!(sirm(10 << 24).alignment().unwrap(), 1024);
        assert!(sirm(40 << 24).alignment().is_err());
    }
}
