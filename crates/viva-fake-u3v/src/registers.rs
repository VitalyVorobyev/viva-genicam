//! In-memory register map and embedded GenApi XML for the fake U3V camera.
//!
//! The bootstrap registers are laid out from the standards, with constants of
//! the fake's own rather than `viva-u3v`'s, so the two can disagree
//! (ADR-0019, backlog TC-27):
//!
//! - **ABRM**: GenCP 1.2, Table 15; capability bits from Table 17; the
//!   manifest from Tables 29-30.
//! - **SBRM**: the offsets a Grasshopper3's XML maps for itself
//!   (`PGR_GS3_U3_23S6M.xml`), and the SIRM address where aravis and
//!   Wireshark agree it is.
//! - **SIRM**: aravis's and Wireshark's layout.
//! - **EIRM**: the three registers Wireshark names.
//! - **Timestamp**: GenCP 1.3.1 §5.4.16-18 — a device clock in ns, latched
//!   by a write.
//!
//! Every register is little-endian: the bootstrap registers because GenCP
//! ties them to USB3 Vision's little-endian framing (§2.3), the camera's own
//! because the Grasshopper3 declares its own so, which the Implementation
//! Endianness register says.

use std::collections::HashMap;

use viva_u3v::registers::{abrm, eirm, sbrm, sirm};

// Feature register addresses (same layout as viva-fake-gige).
pub const REG_WIDTH: u64 = 0x20000;
pub const REG_HEIGHT: u64 = 0x20004;
pub const REG_PIXEL_FORMAT: u64 = 0x20008;
pub const REG_SENSOR_WIDTH: u64 = 0x20014;
pub const REG_SENSOR_HEIGHT: u64 = 0x20018;
pub const REG_ACQ_MODE: u64 = 0x20020;
pub const REG_ACQ_START: u64 = 0x20024;
pub const REG_ACQ_STOP: u64 = 0x20028;
pub const REG_ACQ_FRAME_RATE: u64 = 0x2002c;
pub const REG_EXPOSURE_TIME: u64 = 0x20030;
/// `PayloadSize`, read-only: computed on every read from the current width,
/// height and pixel format's bit depth, packed formats included.
pub const REG_PAYLOAD_SIZE: u64 = 0x20048;
pub const REG_DEVICE_MODEL: u64 = 0x20200;
pub const REG_DEVICE_VENDOR: u64 = 0x20220;
/// Backs `ActionDeviceKey`, an `Integer` that reaches this `WO` register
/// through `<pValue>` -- the shape of [#112]'s `ActionDeviceKey`. USB3 Vision
/// has no action commands; the node is here for its access mode: a write-only
/// feature that is not a command.
///
/// [#112]: https://github.com/VitalyVorobyev/viva-genicam/issues/112
pub const REG_ACTION_DEVICE_KEY: u64 = 0x20040;
/// Backs the `TriggerSoftware` command. Declared `RW`, as 213 of the 490
/// command targets in the vendor corpus are, so the fake serves a read of it
/// -- only [`FakeU3vTransport::reads`](crate::FakeU3vTransport::reads) shows
/// whether a client read a command register it had no reason to read.
pub const REG_TRIGGER_SOFTWARE: u64 = 0x20044;
/// `ChunkModeActive`: 1 sends each frame as chunks.
pub const REG_CHUNK_MODE_ACTIVE: u64 = 0x20050;
/// `ChunkSelector`: which chunk `ChunkEnable` is for.
pub const REG_CHUNK_SELECTOR: u64 = 0x20054;
/// `ChunkEnable`, one register per selector entry, four bytes apart in the
/// selector's order: the timestamp, then the frame ID.
pub const REG_CHUNK_ENABLE: u64 = 0x20058;
/// `EventSelector`: which event `EventNotification` is for.
pub const REG_EVENT_SELECTOR: u64 = 0x20060;
/// `EventNotification` for the one event the fake sends, ExposureEnd.
pub const REG_EVENT_NOTIFICATION: u64 = 0x20064;

/// The image's chunk ID: what a MATRIX VISION mvBlueFOX3-2004G sends, which
/// declares no port for it. Corpus GigE Vision documents declare 1; nothing
/// may depend on either, since the image is the first chunk whatever its ID.
pub const CHUNK_ID_IMAGE: u32 = 0x1122_3344;
/// The fake's chunk carrying the block's device timestamp, 8 bytes.
pub const CHUNK_ID_TIMESTAMP: u32 = 0x0000_1001;
/// The fake's chunk carrying the block id, 8 bytes.
pub const CHUNK_ID_FRAME_ID: u32 = 0x0000_1002;
/// The event sent at the end of each frame while notification is on: in
/// GenCP's device-specific namespace (bits 15-14 = 2, GenCP 1.3.1 Table 2).
pub const EVENT_ID_EXPOSURE_END: u16 = 0x9001;

/// Registers the XML declares `WO`. A read touching any of them is refused
/// with `ACCESS_DENIED`, as a real device refuses it.
///
/// Listed explicitly rather than derived from the XML, so the fake can
/// disagree with our parser (ADR-0019).
pub const WRITE_ONLY_REGISTERS: &[u64] = &[
    REG_ACQ_START,
    REG_ACQ_STOP,
    REG_ACTION_DEVICE_KEY,
    ABRM_TIMESTAMP_LATCH,
];

/// XML blob address in the register space, just past the ABRM's 64 KiB.
pub const XML_BLOB_BASE: u64 = 0x1_0000;

// The bootstrap register maps' offsets are vocabulary, from `viva-u3v`
// (ADR-0030): the ABRM (GenCP 1.2 Table 15) at 0, the SBRM and SIRM where
// this device puts them. What the fake keeps in each, and how it answers, is
// its own reading of the standards.
pub const ABRM_GENCP_VERSION: u64 = abrm::GENCP_VERSION;
pub const ABRM_MANUFACTURER_NAME: u64 = abrm::MANUFACTURER_NAME;
pub const ABRM_MODEL_NAME: u64 = abrm::MODEL_NAME;
pub const ABRM_FAMILY_NAME: u64 = abrm::FAMILY_NAME;
pub const ABRM_DEVICE_VERSION: u64 = abrm::DEVICE_VERSION;
pub const ABRM_MANUFACTURER_INFO: u64 = abrm::MANUFACTURER_INFO;
pub const ABRM_SERIAL_NUMBER: u64 = abrm::SERIAL_NUMBER;
pub const ABRM_USER_DEFINED_NAME: u64 = abrm::USER_DEFINED_NAME;
pub const ABRM_DEVICE_CAPABILITY: u64 = abrm::DEVICE_CAPABILITY;
pub const ABRM_MAX_DEVICE_RESPONSE_TIME: u64 = abrm::MAX_DEVICE_RESPONSE_TIME;
pub const ABRM_MANIFEST_TABLE_ADDRESS: u64 = abrm::MANIFEST_TABLE_ADDRESS;
pub const ABRM_SBRM_ADDRESS: u64 = abrm::SBRM_ADDRESS;
pub const ABRM_DEVICE_CONFIGURATION: u64 = abrm::DEVICE_CONFIGURATION;
pub const ABRM_HEARTBEAT_TIMEOUT: u64 = abrm::HEARTBEAT_TIMEOUT;
pub const ABRM_MESSAGE_CHANNEL_ID: u64 = abrm::MESSAGE_CHANNEL_ID;
pub const ABRM_ACCESS_PRIVILEGE: u64 = abrm::ACCESS_PRIVILEGE;
pub const ABRM_IMPLEMENTATION_ENDIANNESS: u64 = abrm::IMPLEMENTATION_ENDIANNESS;
pub const ABRM_TIMESTAMP: u64 = abrm::TIMESTAMP;
pub const ABRM_TIMESTAMP_LATCH: u64 = abrm::TIMESTAMP_LATCH;
pub const ABRM_TIMESTAMP_INCREMENT: u64 = abrm::TIMESTAMP_INCREMENT;

/// Device capability (GenCP 1.2 Table 17): a user defined name (bit 0), a
/// timestamp (3), UTF-8 strings (bits 4-7 = 1), a family name (8), an SBRM
/// (9), the Implementation Endianness register (10), and the written length
/// in a WRITEMEM_ACK (11).
pub const DEVICE_CAPABILITY: u64 =
    (1 << 0) | (1 << 3) | (1 << 4) | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);

/// Where the SBRM sits.
pub const SBRM_BASE: u64 = 0x1_0000_0000;
pub const SBRM_U3V_VERSION: u64 = SBRM_BASE + sbrm::U3V_VERSION;
pub const SBRM_U3VCP_CAPABILITY: u64 = SBRM_BASE + sbrm::U3VCP_CAPABILITY;
pub const SBRM_U3VCP_CONFIGURATION: u64 = SBRM_BASE + sbrm::U3VCP_CONFIGURATION;
pub const SBRM_MAX_CMD_TRANSFER: u64 = SBRM_BASE + sbrm::MAX_CMD_TRANSFER;
pub const SBRM_MAX_ACK_TRANSFER: u64 = SBRM_BASE + sbrm::MAX_ACK_TRANSFER;
pub const SBRM_NUM_STREAM_CHANNELS: u64 = SBRM_BASE + sbrm::NUM_STREAM_CHANNELS;
pub const SBRM_SIRM_ADDRESS: u64 = SBRM_BASE + sbrm::SIRM_ADDRESS;
pub const SBRM_SIRM_LENGTH: u64 = SBRM_BASE + sbrm::SIRM_LENGTH;
pub const SBRM_EIRM_ADDRESS: u64 = SBRM_BASE + sbrm::EIRM_ADDRESS;
pub const SBRM_EIRM_LENGTH: u64 = SBRM_BASE + sbrm::EIRM_LENGTH;
pub const SBRM_IIDC2_ADDRESS: u64 = SBRM_BASE + sbrm::IIDC2_ADDRESS;
pub const SBRM_CURRENT_SPEED: u64 = SBRM_BASE + sbrm::CURRENT_SPEED;

/// Where the SIRM sits.
pub const SIRM_BASE: u64 = 0x2_0000_0000;
pub const SIRM_INFO: u64 = SIRM_BASE + sirm::INFO;
pub const SIRM_CONTROL: u64 = SIRM_BASE + sirm::CONTROL;
pub const SIRM_REQUIRED_PAYLOAD_SIZE: u64 = SIRM_BASE + sirm::REQUIRED_PAYLOAD_SIZE;
pub const SIRM_REQUIRED_LEADER_SIZE: u64 = SIRM_BASE + sirm::REQUIRED_LEADER_SIZE;
pub const SIRM_REQUIRED_TRAILER_SIZE: u64 = SIRM_BASE + sirm::REQUIRED_TRAILER_SIZE;
pub const SIRM_MAX_LEADER_SIZE: u64 = SIRM_BASE + sirm::MAX_LEADER_SIZE;
pub const SIRM_PAYLOAD_TRANSFER_SIZE: u64 = SIRM_BASE + sirm::PAYLOAD_TRANSFER_SIZE;
pub const SIRM_PAYLOAD_TRANSFER_COUNT: u64 = SIRM_BASE + sirm::PAYLOAD_TRANSFER_COUNT;
pub const SIRM_FINAL_TRANSFER1_SIZE: u64 = SIRM_BASE + sirm::FINAL_TRANSFER1_SIZE;
pub const SIRM_FINAL_TRANSFER2_SIZE: u64 = SIRM_BASE + sirm::FINAL_TRANSFER2_SIZE;
pub const SIRM_MAX_TRAILER_SIZE: u64 = SIRM_BASE + sirm::MAX_TRAILER_SIZE;

/// Where the EIRM sits.
pub const EIRM_BASE: u64 = 0x2_0000_1000;
pub const EIRM_CONTROL: u64 = EIRM_BASE + eirm::CONTROL;
pub const EIRM_MAXIMUM_EVENT_TRANSFER_LENGTH: u64 = EIRM_BASE + eirm::MAXIMUM_EVENT_TRANSFER_LENGTH;
pub const EIRM_EVENT_TEST_CONTROL: u64 = EIRM_BASE + eirm::EVENT_TEST_CONTROL;
/// The longest event command the fake sends.
pub const MAX_EVENT_TRANSFER: u32 = 1024;

/// The SBRM's current speed when nothing says otherwise: bit 3, SuperSpeed
/// (the Grasshopper3's XML reads 8 as SuperSpeed).
pub const SUPERSPEED: u32 = 0b1000;
/// The SBRM's current speed for a USB 2 link: bit 2, high speed.
pub const HIGH_SPEED: u32 = 0b0100;

/// Transfers must be multiples of 2^10 bytes, a SuperSpeed bulk packet: big
/// enough that a small frame's payload does not fill whole transfers, so
/// the final transfer is exercised.
pub const SIRM_ALIGNMENT_EXPONENT: u32 = 10;
/// The leader the fake sends: the 52-byte image leader.
pub const LEADER_SIZE: u32 = 52;
/// The trailer the fake sends: 28 bytes plus the image's size y.
pub const TRAILER_SIZE: u32 = 32;

/// Manifest table address.
pub const MANIFEST_TABLE: u64 = 0x3_0000_0000;

/// What the host has written to the SIRM for reading a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostLayout {
    pub max_leader_size: u32,
    pub payload_transfer_size: u32,
    pub payload_transfer_count: u32,
    pub final_transfer1_size: u32,
    pub final_transfer2_size: u32,
    pub max_trailer_size: u32,
}

/// In-memory register map.
pub struct RegisterMap {
    /// Register blocks by base address. A byte outside every block is not
    /// mapped, and a device refuses to read it.
    regs: HashMap<u64, Vec<u8>>,
    /// `[start, end)` ranges a host write is refused on.
    read_only: Vec<(u64, u64)>,
}

impl RegisterMap {
    pub fn new(width: u32, height: u32, pixel_format: u32) -> Self {
        let xml = generate_xml();
        let mut map = Self {
            regs: HashMap::new(),
            read_only: Vec::new(),
        };

        // -- ABRM --
        map.write_u32(ABRM_GENCP_VERSION, 0x0001_0002); // GenCP 1.2
        map.write_string(ABRM_MANUFACTURER_NAME, "FakeCorp", 64);
        map.write_string(ABRM_MODEL_NAME, "FakeU3V", 64);
        map.write_string(ABRM_FAMILY_NAME, "Test", 64);
        map.write_string(ABRM_DEVICE_VERSION, "1.0.0", 64);
        map.write_string(ABRM_MANUFACTURER_INFO, "in-process fake", 64);
        map.write_string(ABRM_SERIAL_NUMBER, "FAKE-001", 64);
        map.write_string(ABRM_USER_DEFINED_NAME, "", 64);
        map.write_u64(ABRM_DEVICE_CAPABILITY, DEVICE_CAPABILITY);
        map.write_u32(ABRM_MAX_DEVICE_RESPONSE_TIME, 100);
        map.write_u64(ABRM_MANIFEST_TABLE_ADDRESS, MANIFEST_TABLE);
        map.write_u64(ABRM_SBRM_ADDRESS, SBRM_BASE);
        map.write_u64(ABRM_DEVICE_CONFIGURATION, 0);
        map.write_u32(ABRM_HEARTBEAT_TIMEOUT, 3000);
        map.write_u32(ABRM_MESSAGE_CHANNEL_ID, 0);
        map.write_u32(ABRM_ACCESS_PRIVILEGE, 0);
        map.write_u32(ABRM_IMPLEMENTATION_ENDIANNESS, 0xFFFF_FFFF); // little
        map.write_u64(ABRM_TIMESTAMP, 0);
        map.write_u32(ABRM_TIMESTAMP_LATCH, 0);
        map.write_u64(ABRM_TIMESTAMP_INCREMENT, 1); // 1 ns a tick
        // Everything but the user name and the device configuration,
        // heartbeat, message channel and access privilege is the device's.
        map.read_only
            .push((ABRM_GENCP_VERSION, ABRM_USER_DEFINED_NAME));
        map.read_only
            .push((ABRM_DEVICE_CAPABILITY, ABRM_DEVICE_CONFIGURATION));
        map.read_only.push((
            ABRM_IMPLEMENTATION_ENDIANNESS,
            ABRM_IMPLEMENTATION_ENDIANNESS + 4,
        ));
        map.read_only.push((ABRM_TIMESTAMP, ABRM_TIMESTAMP + 8));
        map.read_only
            .push((ABRM_TIMESTAMP_INCREMENT, ABRM_TIMESTAMP_INCREMENT + 8));

        // -- SBRM --
        map.write_u32(SBRM_U3V_VERSION, 0x0001_0000); // U3V 1.0
        map.write_u64(SBRM_U3VCP_CAPABILITY, 0b011); // a SIRM and an EIRM; no IIDC2
        map.write_u64(SBRM_U3VCP_CONFIGURATION, 0);
        map.write_u32(SBRM_MAX_CMD_TRANSFER, 4096);
        map.write_u32(SBRM_MAX_ACK_TRANSFER, 4096);
        map.write_u32(SBRM_NUM_STREAM_CHANNELS, 1);
        map.write_u64(SBRM_SIRM_ADDRESS, SIRM_BASE);
        map.write_u32(SBRM_SIRM_LENGTH, 0x30);
        map.write_u64(SBRM_EIRM_ADDRESS, EIRM_BASE);
        map.write_u32(SBRM_EIRM_LENGTH, 0x0C);
        map.write_u64(SBRM_IIDC2_ADDRESS, 0);
        map.write_u32(SBRM_CURRENT_SPEED, SUPERSPEED);
        map.read_only
            .push((SBRM_U3V_VERSION, SBRM_U3VCP_CONFIGURATION));
        map.read_only
            .push((SBRM_MAX_CMD_TRANSFER, SBRM_CURRENT_SPEED + 4));

        // -- SIRM: Info and the required sizes are the device's --
        map.write_u32(SIRM_INFO, SIRM_ALIGNMENT_EXPONENT << 24);
        map.write_u32(SIRM_CONTROL, 0);
        map.write_u64(SIRM_REQUIRED_PAYLOAD_SIZE, 0); // synced below
        map.write_u32(SIRM_REQUIRED_LEADER_SIZE, LEADER_SIZE);
        map.write_u32(SIRM_REQUIRED_TRAILER_SIZE, TRAILER_SIZE);
        for host in [
            SIRM_MAX_LEADER_SIZE,
            SIRM_PAYLOAD_TRANSFER_SIZE,
            SIRM_PAYLOAD_TRANSFER_COUNT,
            SIRM_FINAL_TRANSFER1_SIZE,
            SIRM_FINAL_TRANSFER2_SIZE,
            SIRM_MAX_TRAILER_SIZE,
        ] {
            map.write_u32(host, 0);
        }
        map.read_only.push((SIRM_INFO, SIRM_CONTROL));
        map.read_only
            .push((SIRM_REQUIRED_PAYLOAD_SIZE, SIRM_MAX_LEADER_SIZE));

        // -- EIRM: the maximum transfer length is the device's --
        map.write_u32(EIRM_CONTROL, 0);
        map.write_u32(EIRM_MAXIMUM_EVENT_TRANSFER_LENGTH, MAX_EVENT_TRANSFER);
        map.write_u32(EIRM_EVENT_TEST_CONTROL, 0);
        map.read_only.push((
            EIRM_MAXIMUM_EVENT_TRANSFER_LENGTH,
            EIRM_MAXIMUM_EVENT_TRANSFER_LENGTH + 4,
        ));

        // -- Manifest, GenCP 1.2 Tables 29-30 --
        map.write_u64(MANIFEST_TABLE, 1);
        let mut entry = vec![0u8; 64];
        entry[0..4].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // file version 1.0.0
        entry[4..8].copy_from_slice(&0u32.to_le_bytes()); // device XML, uncompressed
        entry[8..16].copy_from_slice(&XML_BLOB_BASE.to_le_bytes());
        entry[16..24].copy_from_slice(&(xml.len() as u64).to_le_bytes());
        map.regs.insert(MANIFEST_TABLE + 8, entry);
        map.read_only
            .push((MANIFEST_TABLE, MANIFEST_TABLE + 8 + 64));

        // -- XML blob --
        map.regs.insert(XML_BLOB_BASE, xml.as_bytes().to_vec());
        map.read_only
            .push((XML_BLOB_BASE, XML_BLOB_BASE + xml.len() as u64));

        // -- Feature registers --
        map.write_u32(REG_WIDTH, width);
        map.write_u32(REG_HEIGHT, height);
        map.write_u32(REG_PIXEL_FORMAT, pixel_format);
        map.write_u32(REG_SENSOR_WIDTH, width);
        map.write_u32(REG_SENSOR_HEIGHT, height);
        map.write_u32(REG_ACQ_MODE, 0); // Continuous
        map.write_u32(REG_ACQ_START, 0);
        map.write_u32(REG_ACQ_STOP, 0);
        map.write_u32(REG_ACQ_FRAME_RATE, 200_f32.to_bits());
        map.regs
            .insert(REG_EXPOSURE_TIME, 10000.0f64.to_le_bytes().to_vec());
        map.write_u32(REG_PAYLOAD_SIZE, 0); // synced below
        map.write_string(REG_DEVICE_MODEL, "FakeU3V", 32);
        map.write_string(REG_DEVICE_VENDOR, "FakeCorp", 32);
        map.write_u32(REG_ACTION_DEVICE_KEY, 0);
        map.write_u32(REG_TRIGGER_SOFTWARE, 0);
        map.write_u32(REG_CHUNK_MODE_ACTIVE, 0);
        map.write_u32(REG_CHUNK_SELECTOR, 0);
        map.write_u32(REG_CHUNK_ENABLE, 0);
        map.write_u32(REG_CHUNK_ENABLE + 4, 0);
        map.write_u32(REG_EVENT_SELECTOR, u32::from(EVENT_ID_EXPOSURE_END));
        map.write_u32(REG_EVENT_NOTIFICATION, 0);

        map.sync_payload_size();
        map
    }

    /// Whether every byte of `[addr, addr + len)` is a register.
    pub fn is_mapped(&self, addr: u64, len: usize) -> bool {
        (0..len as u64).all(|i| self.block_of(addr + i).is_some())
    }

    /// Whether `[addr, addr + len)` touches a register the XML declares `WO`.
    pub fn is_write_only(&self, addr: u64, len: usize) -> bool {
        let end = addr.saturating_add(len as u64);
        WRITE_ONLY_REGISTERS
            .iter()
            .any(|&reg| reg < end && addr < reg + 4)
    }

    /// Whether `[addr, addr + len)` touches a register the host may not
    /// write.
    pub fn is_read_only(&self, addr: u64, len: usize) -> bool {
        let end = addr.saturating_add(len as u64);
        self.read_only.iter().any(|&(s, e)| addr < e && s < end)
            || (addr < REG_PAYLOAD_SIZE + 4 && REG_PAYLOAD_SIZE < end)
    }

    fn block_of(&self, a: u64) -> Option<(u64, &Vec<u8>)> {
        self.regs
            .iter()
            .find(|&(&base, data)| a >= base && a < base + data.len() as u64)
            .map(|(&base, data)| (base, data))
    }

    /// The bytes at `[addr, addr + len)`; an unmapped byte reads as 0, though
    /// the transport refuses such a read before it gets here.
    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        (0..len as u64)
            .map(|i| {
                let a = addr + i;
                self.block_of(a)
                    .map(|(base, data)| data[(a - base) as usize])
                    .unwrap_or(0)
            })
            .collect()
    }

    /// Write mapped bytes; the payload sizes follow a change of image format.
    pub fn write(&mut self, addr: u64, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let a = addr + i as u64;
            if let Some(base) = self.block_of(a).map(|(base, _)| base) {
                let block = self.regs.get_mut(&base).expect("block exists");
                block[(a - base) as usize] = byte;
            }
        }
        self.sync_payload_size();
    }

    /// `PayloadSize` and the SIRM's Required Payload Size, from the current
    /// width, height and pixel format.
    fn sync_payload_size(&mut self) {
        let size = self.payload_size();
        self.write_u32(REG_PAYLOAD_SIZE, size as u32);
        self.write_u64(SIRM_REQUIRED_PAYLOAD_SIZE, size);
    }

    /// Bytes in one frame: width × height × the pixel format's bit depth
    /// (PFNC bits 23-16), rounded up to whole bytes — so a packed format such
    /// as `Mono12Packed` takes 1.5 bytes a pixel, as on a real camera — and,
    /// in chunk mode, the image's tag and each enabled chunk with its tag.
    pub fn payload_size(&self) -> u64 {
        let image = payload_bytes(self.width(), self.height(), self.pixel_format_code());
        if !self.chunk_mode() {
            return image;
        }
        image + 8 + 16 * self.enabled_chunks().len() as u64
    }

    /// `AcquisitionFrameRate`, in frames a second.
    pub fn frame_rate(&self) -> f64 {
        f64::from(f32::from_bits(self.read_u32(REG_ACQ_FRAME_RATE)))
    }

    /// Whether `ChunkModeActive` is set.
    pub fn chunk_mode(&self) -> bool {
        self.read_u32(REG_CHUNK_MODE_ACTIVE) & 1 != 0
    }

    /// The chunk IDs enabled, in the selector's order.
    pub fn enabled_chunks(&self) -> Vec<u32> {
        [CHUNK_ID_TIMESTAMP, CHUNK_ID_FRAME_ID]
            .into_iter()
            .enumerate()
            .filter(|&(index, _)| self.read_u32(REG_CHUNK_ENABLE + 4 * index as u64) & 1 != 0)
            .map(|(_, id)| id)
            .collect()
    }

    /// Whether the host has set the EIRM's EventEnable.
    pub fn events_enabled(&self) -> bool {
        self.read_u32(EIRM_CONTROL) & 1 != 0
    }

    /// Whether `EventNotification` is on for ExposureEnd.
    pub fn exposure_end_notification(&self) -> bool {
        self.read_u32(REG_EVENT_NOTIFICATION) == 1
    }

    /// Set the device's clock reading, as a latch does.
    pub fn latch_timestamp(&mut self, ns: u64) {
        self.write_u64(ABRM_TIMESTAMP, ns);
    }

    /// Set the SBRM's current speed register.
    pub fn set_current_speed(&mut self, bits: u32) {
        self.write_u32(SBRM_CURRENT_SPEED, bits);
    }

    pub(crate) fn read_u32(&self, addr: u64) -> u32 {
        u32::from_le_bytes(self.read(addr, 4).try_into().unwrap())
    }

    /// Current image width from the register map.
    pub fn width(&self) -> u32 {
        self.read_u32(REG_WIDTH)
    }

    /// Current image height from the register map.
    pub fn height(&self) -> u32 {
        self.read_u32(REG_HEIGHT)
    }

    /// Current pixel format PFNC code from the register map.
    pub fn pixel_format_code(&self) -> u32 {
        self.read_u32(REG_PIXEL_FORMAT)
    }

    /// Whether the host has set StreamEnable in the SIRM.
    pub fn stream_enabled(&self) -> bool {
        self.read_u32(SIRM_CONTROL) & 1 != 0
    }

    /// The transfer layout the host has written to the SIRM.
    pub fn host_layout(&self) -> HostLayout {
        HostLayout {
            max_leader_size: self.read_u32(SIRM_MAX_LEADER_SIZE),
            payload_transfer_size: self.read_u32(SIRM_PAYLOAD_TRANSFER_SIZE),
            payload_transfer_count: self.read_u32(SIRM_PAYLOAD_TRANSFER_COUNT),
            final_transfer1_size: self.read_u32(SIRM_FINAL_TRANSFER1_SIZE),
            final_transfer2_size: self.read_u32(SIRM_FINAL_TRANSFER2_SIZE),
            max_trailer_size: self.read_u32(SIRM_MAX_TRAILER_SIZE),
        }
    }

    fn write_u32(&mut self, addr: u64, value: u32) {
        self.regs.insert(addr, value.to_le_bytes().to_vec());
    }

    fn write_u64(&mut self, addr: u64, value: u64) {
        self.regs.insert(addr, value.to_le_bytes().to_vec());
    }

    fn write_string(&mut self, addr: u64, s: &str, max_len: usize) {
        let mut data = vec![0u8; max_len];
        let bytes = s.as_bytes();
        let n = bytes.len().min(max_len - 1);
        data[..n].copy_from_slice(&bytes[..n]);
        self.regs.insert(addr, data);
    }
}

fn generate_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription
    ModelName="FakeU3V"
    VendorName="FakeCorp"
    ToolTip="Fake USB3 Vision camera for testing"
    StandardNameSpace="None"
    SchemaMajorVersion="1"
    SchemaMinorVersion="0"
    SchemaSubMinorVersion="0"
    MajorVersion="1"
    MinorVersion="0"
    SubMinorVersion="0"
    ProductGuid="11111111-2222-3333-4444-555555555555"
    VersionGuid="AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"
    xmlns="http://www.genicam.org/GenApi/Version_1_0"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:schemaLocation="http://www.genicam.org/GenApi/Version_1_0 GenApiSchema.xsd">

  <Category Name="Root" NameSpace="Standard">
    <pFeature>DeviceControl</pFeature>
    <pFeature>ImageFormatControl</pFeature>
    <pFeature>AcquisitionControl</pFeature>
    <pFeature>ChunkDataControl</pFeature>
    <pFeature>EventControl</pFeature>
  </Category>

  <Category Name="DeviceControl" NameSpace="Standard">
    <pFeature>DeviceModelName</pFeature>
    <pFeature>DeviceVendorName</pFeature>
    <pFeature>TimestampLatch</pFeature>
    <pFeature>Timestamp</pFeature>
    <pFeature>TimestampIncrement</pFeature>
  </Category>

  <!-- The timestamp features over GenCP's bootstrap registers, named as a
       MATRIX VISION mvBlueFOX3-2004G and a Grasshopper3 name them: the latch
       at 0x1F8, the latched value in ns at 0x1F0, ns a tick at 0x1FC. -->
  <Command Name="TimestampLatch" NameSpace="Standard">
    <pValue>TimestampLatchReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="TimestampLatchReg">
    <Address>0x1F8</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="Timestamp" NameSpace="Standard">
    <pValue>TimestampReg</pValue>
  </Integer>
  <IntReg Name="TimestampReg">
    <Address>0x1F0</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
    <Cachable>NoCache</Cachable>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="TimestampIncrement" NameSpace="Standard">
    <pValue>TimestampIncrementReg</pValue>
  </Integer>
  <IntReg Name="TimestampIncrementReg">
    <Address>0x1FC</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <StringReg Name="DeviceModelName" NameSpace="Standard">
    <Address>0x20200</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
  </StringReg>

  <StringReg Name="DeviceVendorName" NameSpace="Standard">
    <Address>0x20220</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
  </StringReg>

  <Category Name="ImageFormatControl" NameSpace="Standard">
    <pFeature>Width</pFeature>
    <pFeature>Height</pFeature>
    <pFeature>PixelFormat</pFeature>
  </Category>

  <Integer Name="Width" NameSpace="Standard">
    <Min>1</Min>
    <Max>4096</Max>
    <Inc>1</Inc>
    <pValue>WidthReg</pValue>
  </Integer>
  <IntReg Name="WidthReg">
    <Address>0x20000</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Integer Name="Height" NameSpace="Standard">
    <Min>1</Min>
    <Max>4096</Max>
    <Inc>1</Inc>
    <pValue>HeightReg</pValue>
  </Integer>
  <IntReg Name="HeightReg">
    <Address>0x20004</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Enumeration Name="PixelFormat" NameSpace="Standard">
    <EnumEntry Name="Mono8" NameSpace="Standard">
      <Value>17301505</Value>
    </EnumEntry>
    <EnumEntry Name="RGB8Packed" NameSpace="Standard">
      <Value>35127316</Value>
    </EnumEntry>
    <EnumEntry Name="Mono12Packed" NameSpace="Standard">
      <Value>17563719</Value>
    </EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>
  <IntReg Name="PayloadSize" NameSpace="Standard">
    <Address>0x20048</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <IntReg Name="PixelFormatReg">
    <Address>0x20008</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Category Name="AcquisitionControl" NameSpace="Standard">
    <pFeature>AcquisitionMode</pFeature>
    <pFeature>AcquisitionFrameRate</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
    <pFeature>TriggerSoftware</pFeature>
    <pFeature>ActionDeviceKey</pFeature>
  </Category>

  <Enumeration Name="AcquisitionMode" NameSpace="Standard">
    <EnumEntry Name="Continuous"><Value>0</Value></EnumEntry>
    <EnumEntry Name="SingleFrame"><Value>1</Value></EnumEntry>
    <pValue>AcquisitionModeReg</pValue>
  </Enumeration>
  <IntReg Name="AcquisitionModeReg">
    <Address>0x20020</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <!-- The rate blocks start at while acquiring. -->
  <Float Name="AcquisitionFrameRate" NameSpace="Standard">
    <Min>1</Min>
    <Max>1000</Max>
    <pValue>AcquisitionFrameRateReg</pValue>
  </Float>
  <FloatReg Name="AcquisitionFrameRateReg">
    <Address>0x2002C</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Endianess>LittleEndian</Endianess>
  </FloatReg>

  <Command Name="AcquisitionStart" NameSpace="Standard">
    <pValue>AcquisitionStartReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="AcquisitionStartReg">
    <Address>0x20024</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Command Name="AcquisitionStop" NameSpace="Standard">
    <pValue>AcquisitionStopReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="AcquisitionStopReg">
    <Address>0x20028</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <!-- A command whose register is RW: nothing refuses a read of it, so only
       the transport's read log tells whether a client read it. -->
  <Command Name="TriggerSoftware" NameSpace="Standard">
    <pValue>TriggerSoftwareReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="TriggerSoftwareReg">
    <Address>0x20044</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <!-- A write-only feature that is not a command, write-only only through
       the register it delegates to. -->
  <Integer Name="ActionDeviceKey" NameSpace="Standard">
    <Min>0</Min>
    <Max>4294967295</Max>
    <pValue>ActionDeviceKeyReg</pValue>
  </Integer>
  <IntReg Name="ActionDeviceKeyReg">
    <Address>0x20040</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <!-- Chunks: the image first, then each enabled value, every tag
       little-endian. -->
  <Category Name="ChunkDataControl" NameSpace="Standard">
    <pFeature>ChunkModeActive</pFeature>
    <pFeature>ChunkSelector</pFeature>
    <pFeature>ChunkEnable</pFeature>
    <pFeature>ChunkTimestamp</pFeature>
    <pFeature>ChunkFrameID</pFeature>
  </Category>
  <Boolean Name="ChunkModeActive" NameSpace="Standard">
    <pValue>ChunkModeActiveReg</pValue>
    <OnValue>1</OnValue>
    <OffValue>0</OffValue>
  </Boolean>
  <IntReg Name="ChunkModeActiveReg">
    <Address>0x20050</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Enumeration Name="ChunkSelector" NameSpace="Standard">
    <EnumEntry Name="Timestamp" NameSpace="Standard"><Value>0</Value></EnumEntry>
    <EnumEntry Name="FrameID" NameSpace="Standard"><Value>1</Value></EnumEntry>
    <pValue>ChunkSelectorReg</pValue>
    <pSelected>ChunkEnable</pSelected>
  </Enumeration>
  <IntReg Name="ChunkSelectorReg">
    <Address>0x20054</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Boolean Name="ChunkEnable" NameSpace="Standard">
    <pValue>ChunkEnableReg</pValue>
    <OnValue>1</OnValue>
    <OffValue>0</OffValue>
  </Boolean>
  <IntReg Name="ChunkEnableReg">
    <Address>0x20058</Address>
    <pIndex>ChunkSelectorReg</pIndex>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="ChunkTimestamp" NameSpace="Standard">
    <pValue>ChunkTimestampReg</pValue>
  </Integer>
  <IntReg Name="ChunkTimestampReg">
    <Address>0x0</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>TimestampChunkPort</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="ChunkFrameID" NameSpace="Standard">
    <pValue>ChunkFrameIDReg</pValue>
  </Integer>
  <IntReg Name="ChunkFrameIDReg">
    <Address>0x0</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>FrameIDChunkPort</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <!-- Events: GenCP's event layout on the event port — the ID at 0x2, the
       timestamp at 0x4, the data from 0xC — with the data as a MATRIX VISION
       mvBlueFOX3-2004G sends it: the stream channel index (u32) at 0xC, the
       frame ID (u64) at 0x10. -->
  <Category Name="EventControl" NameSpace="Standard">
    <pFeature>EventSelector</pFeature>
    <pFeature>EventNotification</pFeature>
    <pFeature>EventExposureEndData</pFeature>
  </Category>
  <Enumeration Name="EventSelector" NameSpace="Standard">
    <EnumEntry Name="ExposureEnd" NameSpace="Standard"><Value>36865</Value></EnumEntry>
    <pValue>EventSelectorReg</pValue>
    <pSelected>EventNotification</pSelected>
  </Enumeration>
  <IntReg Name="EventSelectorReg">
    <Address>0x20060</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Enumeration Name="EventNotification" NameSpace="Standard">
    <EnumEntry Name="Off" NameSpace="Standard"><Value>0</Value></EnumEntry>
    <EnumEntry Name="On" NameSpace="Standard"><Value>1</Value></EnumEntry>
    <pValue>EventNotificationReg</pValue>
  </Enumeration>
  <IntReg Name="EventNotificationReg">
    <Address>0x20064</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Category Name="EventExposureEndData" NameSpace="Standard">
    <pFeature>EventExposureEnd</pFeature>
    <pFeature>EventExposureEndTimestamp</pFeature>
    <pFeature>EventExposureEndFrameID</pFeature>
  </Category>
  <Integer Name="EventExposureEnd" NameSpace="Standard">
    <pValue>EventExposureEndReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndReg">
    <Address>0x2</Address>
    <Length>2</Length>
    <AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="EventExposureEndTimestamp" NameSpace="Standard">
    <pValue>EventExposureEndTimestampReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndTimestampReg">
    <Address>0x4</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>
  <Integer Name="EventExposureEndFrameID" NameSpace="Standard">
    <pValue>EventExposureEndFrameIDReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndFrameIDReg">
    <Address>0x10</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Port Name="Device" NameSpace="Standard" />
  <Port Name="TimestampChunkPort"><ChunkID>1001</ChunkID></Port>
  <Port Name="FrameIDChunkPort"><ChunkID>1002</ChunkID></Port>
  <Port Name="EventExposureEndPort"><EventID>9001</EventID></Port>
</RegisterDescription>"#
        .to_string()
}

/// Bytes in a `width` × `height` frame of PFNC format `pixel_format`.
pub fn payload_bytes(width: u32, height: u32, pixel_format: u32) -> u64 {
    let bits = u64::from((pixel_format >> 16) & 0xFF);
    (u64::from(width) * u64::from(height) * bits).div_ceil(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bootstrap registers sit where the standards put them, as
    /// little-endian bytes. The offsets here are literals from GenCP 1.2
    /// Table 15 and the Grasshopper3's XML, not the fake's constants.
    #[test]
    fn the_bootstrap_bytes_are_where_the_standards_put_them() {
        let map = RegisterMap::new(640, 480, 0x0108_0001);
        assert_eq!(map.read(0x0000, 4), [0x02, 0x00, 0x01, 0x00]); // GenCP 1.2
        assert_eq!(&map.read(0x0004, 8), b"FakeCorp");
        assert_eq!(&map.read(0x0044, 7), b"FakeU3V");
        assert_eq!(&map.read(0x0144, 8), b"FAKE-001");
        assert_eq!(map.read(0x01CC, 4), 100u32.to_le_bytes());
        assert_eq!(map.read(0x01D0, 8), 0x3_0000_0000u64.to_le_bytes());
        assert_eq!(map.read(0x01D8, 8), 0x1_0000_0000u64.to_le_bytes());

        let sbrm = 0x1_0000_0000u64;
        assert_eq!(map.read(sbrm + 0x14, 4), 4096u32.to_le_bytes());
        assert_eq!(map.read(sbrm + 0x18, 4), 4096u32.to_le_bytes());
        assert_eq!(map.read(sbrm + 0x1C, 4), 1u32.to_le_bytes());
        assert_eq!(map.read(sbrm + 0x20, 8), 0x2_0000_0000u64.to_le_bytes());

        let sirm = 0x2_0000_0000u64;
        assert_eq!(map.read(sirm + 0x08, 8), (640u64 * 480).to_le_bytes());
        assert_eq!(map.read(sirm + 0x10, 4), 52u32.to_le_bytes());
        assert_eq!(map.read(sirm + 0x14, 4), 32u32.to_le_bytes());
    }

    #[test]
    fn the_required_payload_follows_the_image_format() {
        let mut map = RegisterMap::new(64, 48, 0x0108_0001);
        map.write(REG_PIXEL_FORMAT, &0x010C_0047u32.to_le_bytes()); // Mono12Packed
        assert_eq!(
            map.read(SIRM_REQUIRED_PAYLOAD_SIZE, 8),
            4608u64.to_le_bytes()
        );
        assert_eq!(map.read(REG_PAYLOAD_SIZE, 4), 4608u32.to_le_bytes());
    }

    #[test]
    fn the_device_owns_its_required_sizes() {
        let map = RegisterMap::new(64, 48, 0x0108_0001);
        assert!(map.is_read_only(SIRM_REQUIRED_PAYLOAD_SIZE, 8));
        assert!(map.is_read_only(SIRM_REQUIRED_LEADER_SIZE, 4));
        assert!(!map.is_read_only(SIRM_MAX_LEADER_SIZE, 4));
        assert!(!map.is_read_only(SIRM_CONTROL, 4));
        assert!(map.is_read_only(ABRM_SBRM_ADDRESS, 8));
        assert!(!map.is_read_only(ABRM_USER_DEFINED_NAME, 64));
        assert!(!map.is_mapped(0x0228, 8), "the reserved ABRM space");
    }
}
