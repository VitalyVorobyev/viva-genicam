//! USB3 Vision control and stream framing, checked against byte layouts taken
//! from the standards rather than from this crate (ADR-0019, backlog TC-27).
//!
//! Sources, by the evidence hierarchy in `CLAUDE.md`:
//!
//! - **GenCP 1.2** (EMVA, free): the command and acknowledge layout (Tables
//!   4-5), the command ids (Table 7), the ReadMem/WriteMem SCDs (Tables 8-11),
//!   the ABRM (Table 15), and §2.3: "The endianess of all fields in GenCP
//!   protocol packets is technology specific and it must match the endianess
//!   of the bootstrap registers of the device." USB3 Vision's framing is
//!   little-endian, so the bootstrap registers are too.
//! - **Vendor XML**: `PGR_GS3_U3_23S6M.xml` (a Point Grey Grasshopper3 USB3)
//!   maps the SBRM itself — U3VCP capability at +0x04, max command transfer
//!   at +0x14, max acknowledge transfer at +0x18, stream channel count at
//!   +0x1C — all `LittleEndian`, and Maximum Device Response Time at ABRM
//!   0x1CC, `LittleEndian`.
//! - **aravis** (`arvuvcpprivate.h`, `arvuvspprivate.h`) and **Wireshark**
//!   (`packet-u3v.c`), two independent implementations, for what neither the
//!   free standards nor the corpus cover: the `U3VC` prefix on both commands
//!   and acknowledges, the SBRM's SIRM address at +0x20, the SIRM layout, and
//!   the stream leader and trailer. The USB3 Vision standard itself is not
//!   freely available.
//!
//! The device below is written from those tables, not from the client's
//! constants, so the two can disagree.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use viva_u3v::ControlChannel;
use viva_u3v::U3vDevice;
use viva_u3v::U3vError;
use viva_u3v::UsbTransfer;
use viva_u3v::{Abrm, Sbrm};
use viva_u3v::{parse_leader, parse_trailer};

const EP_OUT: u8 = 0x01;
const EP_IN: u8 = 0x81;

// GenCP 1.2 Table 7.
const READMEM_CMD: u16 = 0x0800;
const READMEM_ACK: u16 = 0x0801;
const WRITEMEM_CMD: u16 = 0x0802;
const WRITEMEM_ACK: u16 = 0x0803;
// GenCP 1.2 Table 4: RequestAck is bit 14 of the CCD flags.
const REQUEST_ACK: u16 = 1 << 14;
// "U3VC", little-endian, on commands and acknowledges alike.
const U3VC: [u8; 4] = *b"U3VC";

// GenCP 1.2 Table 6.
const GENCP_SUCCESS: u16 = 0x0000;
const GENCP_NOT_IMPLEMENTED: u16 = 0x8001;
const GENCP_INVALID_PARAMETER: u16 = 0x8002;
const GENCP_INVALID_ADDRESS: u16 = 0x8003;
const GENCP_WRITE_PROTECT: u16 = 0x8004;
const GENCP_INVALID_HEADER: u16 = 0x800E;

/// A device that speaks only GenCP as the tables above define it.
#[derive(Default)]
struct SpecDevice {
    memory: Mutex<BTreeMap<u64, u8>>,
    /// `[start, end)` ranges a write must not touch.
    read_only: Vec<(u64, u64)>,
    writes: Mutex<Vec<(u64, Vec<u8>)>>,
    ack: Mutex<Option<Vec<u8>>>,
}

impl SpecDevice {
    fn put(&mut self, address: u64, bytes: &[u8]) {
        let memory = self.memory.get_mut().unwrap();
        for (i, b) in bytes.iter().enumerate() {
            memory.insert(address + i as u64, *b);
        }
    }

    fn read_only(&mut self, start: u64, len: u64) {
        self.read_only.push((start, start + len));
    }

    fn peek(&self, address: u64, len: usize) -> Vec<u8> {
        let memory = self.memory.lock().unwrap();
        (0..len as u64).map(|i| memory[&(address + i)]).collect()
    }

    fn writes(&self) -> Vec<(u64, Vec<u8>)> {
        self.writes.lock().unwrap().clone()
    }

    fn answer(&self, status: u16, command: u16, request_id: u16, scd: &[u8]) -> Vec<u8> {
        let mut ack = U3VC.to_vec();
        ack.extend_from_slice(&status.to_le_bytes());
        ack.extend_from_slice(&command.to_le_bytes());
        ack.extend_from_slice(&(scd.len() as u16).to_le_bytes());
        ack.extend_from_slice(&request_id.to_le_bytes());
        ack.extend_from_slice(scd);
        ack
    }

    fn execute(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let le16 = |at: usize| u16::from_le_bytes([packet[at], packet[at + 1]]);
        let le64 = |at: usize| u64::from_le_bytes(packet[at..at + 8].try_into().unwrap());
        if packet.len() < 12 || packet[..4] != U3VC {
            return None;
        }
        let (flags, command, length, request_id) = (le16(4), le16(6), le16(8), le16(10));
        if flags & REQUEST_ACK == 0 {
            return None;
        }
        if packet.len() != 12 + length as usize {
            return Some(self.answer(GENCP_INVALID_HEADER, command | 1, request_id, &[]));
        }
        Some(match command {
            READMEM_CMD if length == 12 => {
                let (address, reserved, count) = (le64(12), le16(20), le16(22));
                if reserved != 0 {
                    return Some(self.answer(
                        GENCP_INVALID_PARAMETER,
                        READMEM_ACK,
                        request_id,
                        &[],
                    ));
                }
                let memory = self.memory.lock().unwrap();
                let data: Option<Vec<u8>> = (0..u64::from(count))
                    .map(|i| memory.get(&(address + i)).copied())
                    .collect();
                match data {
                    Some(data) => self.answer(GENCP_SUCCESS, READMEM_ACK, request_id, &data),
                    None => self.answer(GENCP_INVALID_ADDRESS, READMEM_ACK, request_id, &[]),
                }
            }
            WRITEMEM_CMD if length >= 8 => {
                // GenCP §3.1: a write with request_id 0 is refused.
                if request_id == 0 {
                    return Some(self.answer(
                        GENCP_INVALID_PARAMETER,
                        WRITEMEM_ACK,
                        request_id,
                        &[],
                    ));
                }
                let (address, data) = (le64(12), &packet[20..]);
                let end = address + data.len() as u64;
                if self.read_only.iter().any(|&(s, e)| address < e && s < end) {
                    return Some(self.answer(GENCP_WRITE_PROTECT, WRITEMEM_ACK, request_id, &[]));
                }
                let mut memory = self.memory.lock().unwrap();
                for (i, b) in data.iter().enumerate() {
                    memory.insert(address + i as u64, *b);
                }
                self.writes.lock().unwrap().push((address, data.to_vec()));
                self.answer(GENCP_SUCCESS, WRITEMEM_ACK, request_id, &[])
            }
            other => self.answer(GENCP_NOT_IMPLEMENTED, other | 1, request_id, &[]),
        })
    }
}

impl UsbTransfer for SpecDevice {
    fn bulk_write(&self, _: u8, data: &[u8], _: Duration) -> Result<usize, U3vError> {
        *self.ack.lock().unwrap() = self.execute(data);
        Ok(data.len())
    }

    fn bulk_read(&self, _: u8, buf: &mut [u8], _: Duration) -> Result<usize, U3vError> {
        // No acknowledge to send: the host waits out its timeout.
        let ack = self.ack.lock().unwrap().take().ok_or(U3vError::Timeout)?;
        buf[..ack.len()].copy_from_slice(&ack);
        Ok(ack.len())
    }
}

/// A string register's 64 bytes.
fn string64(text: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

const SBRM: u64 = 0x0001_0000;
const SIRM: u64 = 0x0002_0000;
const MANIFEST: u64 = 0x0003_0000;
const XML: u64 = 0x0004_0000;
const XML_TEXT: &[u8] = b"<RegisterDescription/>";

/// A camera's bootstrap registers at the offsets in GenCP 1.2 Table 15 and
/// the USB3 Vision SBRM/SIRM layout, every value little-endian.
fn camera() -> SpecDevice {
    let mut d = SpecDevice::default();
    // ABRM, GenCP 1.2 Table 15. The whole technology-agnostic map is
    // read-only to the host except the user name and the device
    // configuration, heartbeat, message channel and access privilege.
    d.put(0x0000, &0x0001_0002u32.to_le_bytes()); // GenCP 1.2
    d.put(0x0004, &string64("Acme"));
    d.put(0x0044, &string64("Model U3"));
    d.put(0x0084, &string64("Family"));
    d.put(0x00C4, &string64("1.2.3"));
    d.put(0x0104, &string64("info"));
    d.put(0x0144, &string64("SN-0042"));
    d.put(0x0184, &string64("cam-left"));
    // Device capability (Table 17): user defined name (bit 0), family name
    // (bit 8), SBRM (bit 9).
    let capability: u64 = (1 << 0) | (1 << 8) | (1 << 9);
    d.put(0x01C4, &capability.to_le_bytes());
    d.put(0x01CC, &250u32.to_le_bytes()); // maximum device response time, ms
    d.put(0x01D0, &MANIFEST.to_le_bytes());
    d.put(0x01D8, &SBRM.to_le_bytes());
    d.read_only(0x0000, 0x0184);
    d.read_only(0x01C4, 0x1C);
    // SBRM.
    d.put(SBRM, &0x0001_0000u32.to_le_bytes()); // U3V 1.0
    d.put(SBRM + 0x04, &0b011u64.to_le_bytes()); // U3VCP capability: SIRM, EIRM
    d.put(SBRM + 0x0C, &0u64.to_le_bytes()); // U3VCP configuration
    d.put(SBRM + 0x14, &1024u32.to_le_bytes()); // max command transfer
    d.put(SBRM + 0x18, &2048u32.to_le_bytes()); // max acknowledge transfer
    d.put(SBRM + 0x1C, &1u32.to_le_bytes()); // stream channels
    d.put(SBRM + 0x20, &SIRM.to_le_bytes());
    d.put(SBRM + 0x28, &0x30u32.to_le_bytes());
    d.put(SBRM + 0x2C, &0u64.to_le_bytes()); // EIRM address
    d.put(SBRM + 0x34, &0u32.to_le_bytes());
    d.read_only(SBRM, 0x0C);
    d.read_only(SBRM + 0x14, 0x30);
    // SIRM: info and the three required sizes are the device's; the rest
    // is what the host configures.
    d.put(SIRM, &(3u32 << 24).to_le_bytes()); // alignment 2^3 = 8 bytes
    d.put(SIRM + 0x04, &0u32.to_le_bytes()); // control
    d.put(SIRM + 0x08, &10_000u64.to_le_bytes()); // required payload
    d.put(SIRM + 0x10, &52u32.to_le_bytes()); // required leader
    d.put(SIRM + 0x14, &32u32.to_le_bytes()); // required trailer
    for offset in [0x18, 0x1C, 0x20, 0x24, 0x28, 0x2C] {
        d.put(SIRM + offset, &0u32.to_le_bytes());
    }
    d.read_only(SIRM, 0x04);
    d.read_only(SIRM + 0x08, 0x10);
    // Manifest, GenCP 1.2 Tables 29-30: a 64-bit entry count, then 64-byte
    // entries.
    d.put(MANIFEST, &1u64.to_le_bytes());
    let mut entry = [0u8; 64];
    entry[0..4].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // file version 1.0.0
    entry[4..8].copy_from_slice(&0u32.to_le_bytes()); // device XML, uncompressed
    entry[8..16].copy_from_slice(&XML.to_le_bytes());
    entry[16..24].copy_from_slice(&(XML_TEXT.len() as u64).to_le_bytes());
    d.put(MANIFEST + 8, &entry);
    d.put(XML, XML_TEXT);
    d
}

fn channel(device: &Arc<SpecDevice>) -> ControlChannel<SpecDevice> {
    ControlChannel::new(Arc::clone(device), EP_IN, EP_OUT, 1024, 1024)
}

/// Records what the host sends, and answers nothing.
#[derive(Default)]
struct Wiretap(Mutex<Vec<Vec<u8>>>);

impl UsbTransfer for Wiretap {
    fn bulk_write(&self, _: u8, data: &[u8], _: Duration) -> Result<usize, U3vError> {
        self.0.lock().unwrap().push(data.to_vec());
        Ok(data.len())
    }
    fn bulk_read(&self, _: u8, _: &mut [u8], _: Duration) -> Result<usize, U3vError> {
        Err(U3vError::Timeout)
    }
}

#[test]
fn a_readmem_command_is_gencp_on_the_wire() {
    let tap = Arc::new(Wiretap::default());
    let mut control = ControlChannel::new(Arc::clone(&tap), EP_IN, EP_OUT, 1024, 1024);
    let _ = control.read_mem(0x01D8, 8);

    #[rustfmt::skip]
    let expected: &[u8] = &[
        b'U', b'3', b'V', b'C',      // prefix
        0x00, 0x40,                  // flags: RequestAck (bit 14)
        0x00, 0x08,                  // command_id: READMEM_CMD 0x0800
        0x0C, 0x00,                  // length: 12
        0x01, 0x00,                  // request_id
        0xD8, 0x01, 0, 0, 0, 0, 0, 0, // register address 0x1D8
        0x00, 0x00,                  // reserved
        0x08, 0x00,                  // read length 8
    ];
    assert_eq!(tap.0.lock().unwrap()[0], expected);
}

#[test]
fn a_writemem_command_is_gencp_on_the_wire_and_never_uses_request_id_zero() {
    let tap = Arc::new(Wiretap::default());
    let mut control = ControlChannel::new(Arc::clone(&tap), EP_IN, EP_OUT, 1024, 1024);
    let _ = control.write_mem(SIRM + 0x04, &1u32.to_le_bytes());

    #[rustfmt::skip]
    let expected: &[u8] = &[
        b'U', b'3', b'V', b'C',
        0x00, 0x40,                  // RequestAck
        0x02, 0x08,                  // WRITEMEM_CMD 0x0802
        0x0C, 0x00,                  // 8 address + 4 data
        0x01, 0x00,                  // GenCP §3.1: never a write with id 0
        0x04, 0x00, 0x02, 0, 0, 0, 0, 0, // SIRM + 4
        0x01, 0x00, 0x00, 0x00,      // data, sent as given
    ];
    assert_eq!(tap.0.lock().unwrap()[0], expected);
}

#[test]
fn reads_and_writes_complete_against_a_conformant_device() {
    let device = Arc::new(camera());
    let mut control = channel(&device);

    assert_eq!(control.read_mem(0x01D8, 8).unwrap(), SBRM.to_le_bytes());
    control
        .write_mem(SIRM + 0x18, &64u32.to_le_bytes())
        .expect("a WRITEMEM_ACK is the answer to a WRITEMEM_CMD");
    assert_eq!(device.peek(SIRM + 0x18, 4), 64u32.to_le_bytes());
}

#[test]
fn the_abrm_is_read_at_the_gencp_offsets_little_endian() {
    let device = Arc::new(camera());
    let abrm = Abrm::read_from(&mut channel(&device)).expect("ABRM");

    assert_eq!(abrm.gencp_version, 0x0001_0002);
    assert_eq!(abrm.manufacturer_name, "Acme");
    assert_eq!(abrm.model_name, "Model U3");
    assert_eq!(abrm.device_version, "1.2.3");
    assert_eq!(abrm.serial_number, "SN-0042");
    assert_eq!(abrm.manifest_table_address, MANIFEST);
    assert_eq!(abrm.sbrm_address, SBRM);
}

#[test]
fn the_sbrm_is_read_at_the_u3v_offsets_little_endian() {
    let device = Arc::new(camera());
    let sbrm = Sbrm::read_from(&mut channel(&device), SBRM).expect("SBRM");

    assert_eq!(sbrm.u3v_version, 0x0001_0000);
    assert_eq!(sbrm.max_cmd_transfer, 1024);
    assert_eq!(sbrm.max_ack_transfer, 2048);
    assert_eq!(sbrm.num_stream_channels, 1);
    assert_eq!(sbrm.sirm_address, SIRM);
}

#[test]
fn the_device_opens_and_hands_over_its_xml() {
    let device = Arc::new(camera());
    let mut camera =
        U3vDevice::open(Arc::clone(&device), EP_IN, EP_OUT, Some(0x82), None).expect("bootstrap");
    assert_eq!(camera.fetch_xml().unwrap().as_bytes(), XML_TEXT);
}

/// The host writes the transfer layout and StreamEnable, never the sizes the
/// device requires, and clears StreamEnable on stop (Codex review of #181).
#[test]
fn a_stream_configures_the_host_registers_and_stops_disabled() {
    let device = Arc::new(camera());
    let mut camera =
        U3vDevice::open(Arc::clone(&device), EP_IN, EP_OUT, Some(0x82), None).expect("bootstrap");

    let _stream = camera.open_stream(10_000).expect("SIRM accepts its layout");
    let written: Vec<u64> = device.writes().iter().map(|(a, _)| a - SIRM).collect();
    for required in [0x08, 0x10, 0x14] {
        assert!(
            !written.contains(&required),
            "wrote a required size: {written:x?}"
        );
    }
    assert_eq!(
        device.peek(SIRM + 0x04, 4),
        1u32.to_le_bytes(),
        "StreamEnable"
    );
    // Leader and trailer are the required sizes, aligned up.
    assert_eq!(device.peek(SIRM + 0x18, 4), 56u32.to_le_bytes());
    assert_eq!(device.peek(SIRM + 0x2C, 4), 32u32.to_le_bytes());
    // The payload's transfers cover the 10 000 required bytes.
    let le32 = |offset| u32::from_le_bytes(device.peek(SIRM + offset, 4).try_into().unwrap());
    let covered = le32(0x1C) * le32(0x20) + le32(0x24) + le32(0x28);
    assert!(covered >= 10_000, "transfers cover {covered} bytes");

    camera.stop_stream().expect("stop");
    assert_eq!(
        device.peek(SIRM + 0x04, 4),
        0u32.to_le_bytes(),
        "StreamEnable cleared"
    );
}

/// A stream leader as `packet-u3v.c` dissects it and `ArvUvspLeader` lays it
/// out: an 8-byte block id at +8 precedes the payload type.
#[test]
fn a_leader_is_read_at_the_u3v_offsets() {
    #[rustfmt::skip]
    let leader: Vec<u8> = [
        &b"U3VL"[..],
        &[0, 0], &52u16.to_le_bytes(),             // reserved, leader size
        &7u64.to_le_bytes(),                       // block id
        &[0, 0], &0x0001u16.to_le_bytes(),         // reserved, payload type: image
        &123_456u64.to_le_bytes(),                 // timestamp
        &0x0108_0001u32.to_le_bytes(),             // Mono8
        &640u32.to_le_bytes(), &480u32.to_le_bytes(),
        &16u32.to_le_bytes(), &8u32.to_le_bytes(), // offsets
        &4u16.to_le_bytes(), &[0, 0],              // x padding, reserved
    ]
    .concat();

    let parsed = parse_leader(&leader).expect("leader");
    assert_eq!(parsed.payload_type, 0x0001);
    assert_eq!(parsed.timestamp, 123_456);
    assert_eq!(parsed.pixel_format, 0x0108_0001);
    assert_eq!((parsed.width, parsed.height), (640, 480));
    assert_eq!((parsed.x_offset, parsed.y_offset), (16, 8));
    assert_eq!(parsed.x_padding, 4);
}

/// A stream trailer as `packet-u3v.c` dissects it: the block id at +8, a
/// 16-bit status at +16, the valid payload size at +20.
#[test]
fn a_trailer_is_read_at_the_u3v_offsets() {
    #[rustfmt::skip]
    let trailer: Vec<u8> = [
        &b"U3VT"[..],
        &[0, 0], &32u16.to_le_bytes(),             // reserved, trailer size
        &7u64.to_le_bytes(),                       // block id
        &0xA101u16.to_le_bytes(), &[0, 0],         // status, reserved
        &9_000u64.to_le_bytes(),                   // valid payload size
        &480u32.to_le_bytes(),                     // size y
    ]
    .concat();

    let parsed = parse_trailer(&trailer).expect("trailer");
    assert_eq!(parsed.block_id, 7);
    assert_eq!(parsed.status, 0xA101);
    assert_eq!(parsed.valid_payload_size, 9_000);
}

/// A device that answers `status` to every command.
struct Refusing(u16, Mutex<Option<Vec<u8>>>);

impl UsbTransfer for Refusing {
    fn bulk_write(&self, _: u8, data: &[u8], _: Duration) -> Result<usize, U3vError> {
        let request_id = u16::from_le_bytes([data[10], data[11]]);
        let device = SpecDevice::default();
        *self.1.lock().unwrap() = Some(device.answer(self.0, READMEM_ACK, request_id, &[]));
        Ok(data.len())
    }

    fn bulk_read(&self, _: u8, buf: &mut [u8], _: Duration) -> Result<usize, U3vError> {
        let ack = self.1.lock().unwrap().take().ok_or(U3vError::Timeout)?;
        buf[..ack.len()].copy_from_slice(&ack);
        Ok(ack.len())
    }
}

fn read_error(status: u16) -> String {
    let device = Arc::new(Refusing(status, Mutex::new(None)));
    let mut control = ControlChannel::new(device, EP_IN, EP_OUT, 1024, 1024);
    control
        .read_mem(0x01D8, 8)
        .expect_err("the device refused the read")
        .to_string()
}

/// GenCP 1.2 Table 6: `0x800B` is `MSG_TIMEOUT` over GenCP, where GigE
/// Vision calls the same number `NO_MSG` (ADR-0020, backlog TC-16).
#[test]
fn gencp_names_0x800b_msg_timeout() {
    let message = read_error(0x800B);
    assert!(message.contains("MSG_TIMEOUT (0x800B)"), "{message}");
}

/// A code only USB3 Vision defines is named, not reported as unknown.
#[test]
fn u3v_names_its_own_codes() {
    let message = read_error(0xA002);
    assert!(
        message.contains("DSI_ENDPOINT_HALTED (0xA002)"),
        "{message}"
    );
}

/// The shared core reads as it does on GigE Vision.
#[test]
fn u3v_names_the_shared_codes() {
    let message = read_error(GENCP_WRITE_PROTECT);
    assert!(message.contains("WRITE_PROTECT (0x8004)"), "{message}");
}
