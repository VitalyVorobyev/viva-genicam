//! Registers on chunk and event ports (GA-12, GA-13).
//!
//! GenICam Standard v2.1.1 §2.8.16: a `<Port>` with a `<ChunkID>` "does not
//! give access to a real device, but rather to the chunk of data residing in
//! memory", and a register names its port with `<pPort>`. Event ports are the
//! same idea for the data an event message carries, keyed by `<EventID>`.
//!
//! Every fixture is a cut-down copy of a shape the vendor corpus ships, with
//! the document cited. Expected values are literals worked out from the bytes
//! in the test, never from the code under test.

use std::cell::RefCell;
use std::collections::HashMap;

use bytes::Bytes;
use viva_genapi::{ChunkData, FeatureValue, GenApiError, NodeMap, PortKind, RegisterIo};

/// Device memory that records every access. A read of an address nothing
/// wrote returns a recognisable pattern, so a value that came from the device
/// cannot pass for one that came from a chunk.
#[derive(Default)]
struct MemIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
    reads: RefCell<Vec<(u64, usize)>>,
    writes: RefCell<Vec<(u64, Vec<u8>)>>,
}

impl RegisterIo for MemIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        self.reads.borrow_mut().push((addr, len));
        Ok(self
            .regs
            .borrow()
            .get(&addr)
            .cloned()
            .unwrap_or_else(|| vec![0xEE; len]))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        self.writes.borrow_mut().push((addr, data.to_vec()));
        Ok(())
    }
}

impl MemIo {
    fn reads(&self) -> Vec<(u64, usize)> {
        self.reads.borrow().clone()
    }

    fn writes(&self) -> Vec<(u64, Vec<u8>)> {
        self.writes.borrow().clone()
    }
}

/// Chunk and event shapes from the corpus:
///
/// - `ChunkTimestamp`: FLIR BFS (`FLIR_BFS_PGE_31S4C_C.xml`) — an `<Integer>`
///   over a little-endian 8-byte `<IntReg>` at offset 0 of its chunk.
/// - `ChunkImageLength`: Baumer HXG20 — `<Address>-4</Address>` on a chunk
///   port, the only negative address in the corpus.
/// - `EventExposureEndTimestamp` / `EventExposureEndFrameID`: FLIR BFS — the
///   timestamp at 0x8 big-endian, the frame id at 0x10 little-endian, on the
///   port whose `<EventID>` is `9C43`.
/// - `EventExposureEnd`: Hikrobot / FLIR BFLY — the event identifier itself,
///   2 bytes at 0x2.
/// - `EventSerialData`: FLIR BFS — a `<StringReg>` on an event port.
/// - `ChunkGainRaw`: not in the corpus (every corpus port register is `RO`);
///   an `RW` register on a chunk port, to pin that writes are refused anyway.
/// - `Width`: a device-port register, the control.
const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Port Name="Device" NameSpace="Standard"/>

  <Integer Name="Width"><pValue>WidthReg</pValue></Integer>
  <IntReg Name="WidthReg">
    <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <pPort>Device</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>

  <Integer Name="ChunkTimestamp">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkTimestamp_Val</pValue>
  </Integer>
  <IntReg Name="ChunkTimestamp_Val">
    <Address>0x00</Address><Length>8</Length><AccessMode>RO</AccessMode>
    <pPort>TimestampPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess>
  </IntReg>
  <Port Name="TimestampPort"><ChunkID>0508000E</ChunkID></Port>

  <IntReg Name="ChunkImageLength" NameSpace="Custom">
    <Address>-4</Address><Length>4</Length><AccessMode>RO</AccessMode>
    <pPort>PortChunkFreeformatHeader3</pPort><Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Port Name="PortChunkFreeformatHeader3" NameSpace="Custom"><ChunkID>00000002</ChunkID></Port>

  <IntReg Name="ChunkGainRaw">
    <Address>0x0</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <pPort>GainPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess>
  </IntReg>
  <Port Name="GainPort"><ChunkID>05200009</ChunkID></Port>

  <Integer Name="EventExposureEndTimestamp">
    <pValue>EventExposureEndTimestamp_Val</pValue>
  </Integer>
  <IntReg Name="EventExposureEndTimestamp_Val">
    <Address>0x8</Address><Length>8</Length><AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>
  <IntReg Name="EventExposureEndFrameID_Val">
    <Address>0x10</Address><Length>8</Length><AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess>
  </IntReg>
  <IntReg Name="EventExposureEnd">
    <Address>0x2</Address><Length>2</Length><AccessMode>RO</AccessMode>
    <pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>
  <Port Name="EventExposureEndPort"><EventID>9C43</EventID></Port>

  <IntSwissKnife Name="ChunkTimestampUs">
    <pVariable Name="T">ChunkTimestamp</pVariable>
    <Formula>T / 1000</Formula>
  </IntSwissKnife>

  <Category Name="ChunkDataControl">
    <pFeature>ChunkTimestamp</pFeature>
    <pFeature>ChunkTimestampUs</pFeature>
    <pFeature>Width</pFeature>
  </Category>
  <Category Name="EventControl">
    <pFeature>EventExposureEndTimestamp</pFeature>
  </Category>

  <StringReg Name="EventSerialData">
    <Address>0x18</Address><Length>8</Length><AccessMode>RO</AccessMode>
    <pPort>EventSerialPortReceivePort</pPort>
  </StringReg>
  <Port Name="EventSerialPortReceivePort"><EventID>9C45</EventID></Port>
</RegisterDescription>
"#;

fn nodemap() -> NodeMap {
    let model = viva_genapi_xml::parse(XML).expect("parse");
    NodeMap::try_from_xml(model).expect("build")
}

/// The device control: a register on the device port still reads the device.
#[test]
fn a_device_port_register_reads_the_device() {
    let map = nodemap();
    let io = MemIo::default();
    io.regs
        .borrow_mut()
        .insert(0x100, 640u32.to_be_bytes().to_vec());
    assert_eq!(map.get_integer("Width", &io).expect("width"), 640);
    assert_eq!(io.reads(), vec![(0x100, 4)]);
}

/// GA-12: with no chunk attached, a chunk-port register is an error — and the
/// device is not asked. Reading it through the device port would return
/// whatever the device keeps at 0x0, which on GigE Vision is the bootstrap
/// `Version` register.
#[test]
fn a_detached_chunk_port_register_is_an_error_not_a_device_read() {
    let map = nodemap();
    let io = MemIo::default();
    let result = map.get_integer("ChunkTimestamp", &io);
    assert!(
        result.is_err(),
        "a chunk feature read a value with nothing attached: {result:?}"
    );
    assert_eq!(io.reads(), vec![], "a chunk feature reached device memory");
}

/// GA-12, event ports: `EventExposureEndTimestamp` at 0x8 is the event's
/// timestamp, not the bootstrap MAC address register that sits at device
/// address 0x8.
#[test]
fn a_detached_event_port_register_is_an_error_not_a_device_read() {
    let map = nodemap();
    let io = MemIo::default();
    let result = map.get_integer("EventExposureEndTimestamp", &io);
    assert!(
        result.is_err(),
        "an event feature read a value with no event attached: {result:?}"
    );
    let string = map.get_string("EventSerialData", &io);
    assert!(
        string.is_err(),
        "an event string read a value with no event attached: {string:?}"
    );
    assert_eq!(io.reads(), vec![], "an event feature reached device memory");
}

/// #112's `BAD_ALIGNMENT`: a 2-byte event register at address 0x2 sent to the
/// device as a READMEM, which requires 4-byte alignment. With ports routed, no
/// such read is sent at all.
#[test]
fn an_unaligned_event_register_is_not_sent_to_the_device() {
    let map = nodemap();
    let io = MemIo::default();
    assert!(map.get_integer("EventExposureEnd", &io).is_err());
    assert_eq!(io.reads(), vec![]);
}

/// A chunk port holds data the camera sent; writing it would change nothing
/// the camera sees. Refused, even for a register declared `RW`, and nothing
/// reaches the device.
#[test]
fn a_chunk_port_register_is_never_written() {
    let mut map = nodemap();
    let io = MemIo::default();
    let result = map.set_integer("ChunkGainRaw", 7, &io);
    assert!(result.is_err(), "a chunk-port write succeeded: {result:?}");
    assert_eq!(io.writes(), vec![], "a chunk-port write reached the device");
}

/// GA-13: `<Address>-4</Address>` parses. Baumer HXG20's `ChunkImageLength`
/// was the one node in the corpus this crate could not build.
#[test]
fn a_negative_address_on_a_chunk_port_is_built() {
    let map = nodemap();
    assert!(
        map.node("ChunkImageLength").is_some(),
        "skipped: {:?}",
        map.skipped()
    );
}

// ---------------------------------------------------------------------------
// Attached data (ADR-0026)
// ---------------------------------------------------------------------------

fn chunks(list: &[(u64, &[u8])]) -> ChunkData {
    list.iter()
        .map(|(id, data)| (*id, Bytes::copy_from_slice(data)))
        .collect()
}

/// `ChunkTimestamp` is 8 bytes little-endian at offset 0 of chunk
/// `0x0508000E` — so it reads from the chunk, and the device is not asked.
#[test]
fn a_chunk_feature_reads_its_chunk() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[(
        0x0508_000E,
        &[0x89, 0x67, 0x45, 0x23, 0x01, 0x00, 0x00, 0x00],
    )]));
    assert_eq!(
        map.get_integer("ChunkTimestamp", &io).expect("attached"),
        0x0000_0001_2345_6789
    );
    assert_eq!(io.reads(), vec![]);
}

/// Cached values never outlive their buffer: the next frame's chunk reads
/// fresh, through the `<pValue>` chain and through a formula, and after a
/// detach nothing is left to read.
#[test]
fn attaching_new_chunks_invalidates_what_was_read_from_the_old_ones() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[(0x0508_000E, &5_000u64.to_le_bytes())]));
    assert_eq!(
        map.get_integer("ChunkTimestamp", &io).expect("first"),
        5_000
    );
    assert_eq!(map.get_integer("ChunkTimestampUs", &io).expect("first"), 5);

    map.attach_chunks(&chunks(&[(0x0508_000E, &9_000u64.to_le_bytes())]));
    assert_eq!(
        map.get_integer("ChunkTimestamp", &io).expect("second"),
        9_000
    );
    assert_eq!(map.get_integer("ChunkTimestampUs", &io).expect("second"), 9);

    map.detach_chunks();
    assert!(matches!(
        map.get_integer("ChunkTimestamp", &io),
        Err(GenApiError::NotAttached { ref port, .. }) if port == "TimestampPort"
    ));
    assert!(map.get_integer("ChunkTimestampUs", &io).is_err());
    assert_eq!(io.reads(), vec![]);
}

/// A chunk the frame does not carry is not attached, even when others are.
#[test]
fn a_chunk_missing_from_the_frame_is_not_attached() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[(0x0000_0002, &[0; 8])]));
    assert!(matches!(
        map.get_integer("ChunkTimestamp", &io),
        Err(GenApiError::NotAttached { .. })
    ));
}

/// GA-13: `-4` is "offset from the back of the chunk" (GenICam Standard
/// v2.1.1 §2.8.16) — the last four bytes of chunk 2, big-endian.
#[test]
fn a_negative_address_reads_from_the_end_of_its_chunk() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[(
        0x0000_0002,
        &[0xAA, 0xBB, 0xCC, 0xDD, 0x00, 0x10, 0x00, 0x00],
    )]));
    assert_eq!(
        map.get_integer("ChunkImageLength", &io).expect("attached"),
        0x0010_0000
    );
    assert_eq!(io.reads(), vec![]);
}

/// A chunk shorter than its register is an error naming the offset, not a
/// panic and not a short read.
#[test]
fn a_chunk_too_short_for_its_register_is_out_of_range() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[
        (0x0508_000E, &[1, 2, 3, 4]),
        (0x0000_0002, &[1, 2]),
    ]));
    assert!(matches!(
        map.get_integer("ChunkTimestamp", &io),
        Err(GenApiError::PortOutOfRange {
            offset: 0,
            len: 8,
            size: 4,
            ..
        })
    ));
    assert!(matches!(
        map.get_integer("ChunkImageLength", &io),
        Err(GenApiError::PortOutOfRange {
            offset: -4,
            len: 4,
            size: 2,
            ..
        })
    ));
}

/// Writing a chunk register is refused whether or not a chunk is attached,
/// and the attached data is not changed.
#[test]
fn a_chunk_port_register_is_not_written_with_a_chunk_attached() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_chunks(&chunks(&[(0x0520_0009, &[1, 0, 0, 0])]));
    assert!(matches!(
        map.set_integer("ChunkGainRaw", 7, &io),
        Err(GenApiError::PortNotWritable { ref port, .. }) if port == "GainPort"
    ));
    assert_eq!(map.get_integer("ChunkGainRaw", &io).expect("attached"), 1);
    assert_eq!(io.writes(), vec![]);
}

/// A GigE Vision `EVENTDATA` event as the device sends it, 16-bit block ids:
/// reserved | event id 0x9C43 | stream channel 0 | block id 0x0102 |
/// timestamp 0x0000_0012_3456_789A | data: frame id 7, little-endian u64.
#[rustfmt::skip]
const EXPOSURE_END: [u8; 24] = [
    0x00, 0x00, 0x9C, 0x43, 0x00, 0x00, 0x01, 0x02,
    0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9A,
    0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// GA-39: an event attaches to the port whose `<EventID>` matches, and its
/// registers address the event from its first byte — header included, which
/// is where every corpus document puts the timestamp (0x8) and event id (0x2).
#[test]
fn an_event_feature_reads_the_event_with_its_id() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_event(0x9C43, Bytes::from_static(&EXPOSURE_END));
    assert_eq!(
        map.get_integer("EventExposureEndTimestamp", &io)
            .expect("ts"),
        0x0000_0012_3456_789A
    );
    assert_eq!(
        map.get_integer("EventExposureEndFrameID_Val", &io)
            .expect("frame id"),
        7
    );
    assert_eq!(
        map.get_integer("EventExposureEnd", &io).expect("id"),
        0x9C43
    );
    // Another event's port has nothing attached.
    assert!(matches!(
        map.get_string("EventSerialData", &io),
        Err(GenApiError::NotAttached { ref port, .. }) if port == "EventSerialPortReceivePort"
    ));
    assert_eq!(io.reads(), vec![]);
}

/// Each event ID keeps its last event: a second ExposureEnd replaces the
/// first, and an event of another kind leaves it alone.
#[test]
fn a_new_event_replaces_only_its_own_kind() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_event(0x9C43, Bytes::from_static(&EXPOSURE_END));
    assert_eq!(
        map.get_integer("EventExposureEndTimestamp", &io)
            .expect("first"),
        0x0000_0012_3456_789A
    );

    let mut later = EXPOSURE_END;
    later[8..16].copy_from_slice(&0x0000_0099_0000_0001u64.to_be_bytes());
    map.attach_event(0x9C43, Bytes::copy_from_slice(&later));
    assert_eq!(
        map.get_integer("EventExposureEndTimestamp", &io)
            .expect("second"),
        0x0000_0099_0000_0001
    );

    let mut serial = vec![0u8; 0x18];
    serial.extend_from_slice(b"hello\0\0\0");
    map.attach_event(0x9C45, Bytes::from(serial));
    assert_eq!(
        map.get_string("EventSerialData", &io).expect("serial"),
        "hello"
    );
    assert_eq!(
        map.get_integer("EventExposureEndTimestamp", &io)
            .expect("kept"),
        0x0000_0099_0000_0001
    );

    map.detach_events();
    assert!(map.get_integer("EventExposureEndTimestamp", &io).is_err());
}

/// An event port has no "back": a negative offset there is out of range, and
/// so is reading past the event.
#[test]
fn an_event_too_short_for_its_register_is_out_of_range() {
    let mut map = nodemap();
    let io = MemIo::default();
    map.attach_event(0x9C43, Bytes::from_static(&EXPOSURE_END[..12]));
    assert!(matches!(
        map.get_integer("EventExposureEndTimestamp", &io),
        Err(GenApiError::PortOutOfRange {
            offset: 8,
            len: 8,
            size: 12,
            ..
        })
    ));
}

/// What a viewer polls between frames: a chunk feature with nothing attached
/// is unavailable and has no value — not an error for every feature on
/// every poll.
#[test]
fn feature_state_reports_a_detached_chunk_feature_as_unavailable() {
    let mut map = nodemap();
    let io = MemIo::default();
    let state = map.feature_state("ChunkTimestamp", &io).expect("state");
    assert_eq!(state.value, FeatureValue::None);
    assert!(!state.is_available);
    assert_eq!(state.access, None);

    map.attach_chunks(&chunks(&[(0x0508_000E, &42u64.to_le_bytes())]));
    let state = map.feature_state("ChunkTimestamp", &io).expect("state");
    assert_eq!(state.value, FeatureValue::Integer(42));
    assert!(state.is_available);
}

/// Introspection: the ports by kind, and the features each one feeds — the
/// category members, not the registers behind them.
#[test]
fn ports_and_the_features_they_feed() {
    let map = nodemap();
    let ports = map.ports();
    assert!(ports.contains(&(
        "TimestampPort",
        PortKind::Chunk {
            chunk_id: 0x0508_000E
        }
    )));
    assert!(ports.contains(&("EventExposureEndPort", PortKind::Event { event_id: 0x9C43 })));
    assert!(ports.contains(&("Device", PortKind::Device)));
    assert_eq!(
        map.port_features("TimestampPort"),
        vec!["ChunkTimestamp", "ChunkTimestampUs"]
    );
    assert_eq!(
        map.port_features("EventExposureEndPort"),
        vec!["EventExposureEndTimestamp"]
    );
    assert!(map.port_features("Device").is_empty());
}

/// A chunk-relative address is not a device address, so `register_address`
/// does not hand one out.
#[test]
fn register_address_refuses_a_chunk_register() {
    let map = nodemap();
    let io = MemIo::default();
    assert!(matches!(
        map.register_address("ChunkTimestamp_Val", &io),
        Err(GenApiError::NotDeviceMemory { .. })
    ));
    assert_eq!(
        map.register_address("WidthReg", &io).expect("device"),
        (0x100, 4)
    );
}

/// The Lucid TRT009S's `EventExposureStartFrameID` (#138), cut down: its
/// register `N248` takes both address and length from IntSwissKnifes on the
/// extended-ID bit `N466` (`<Bit>1</Bit>` of big-endian 0x0960), so the frame
/// id is the 16-bit block id at 0x6 or the 64-bit one at 0x8 (GenICam
/// Standard v2.1.1 §2.8.3's `<pLength>`). `Scratch` is the same `<pLength>`
/// on a device-port register, which can be written.
const TRT009S_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Port Name="Device"/>
  <Integer Name="EventExposureStartFrameID"><pValue>N248</pValue></Integer>
  <IntReg Name="N248"><pAddress>N256</pAddress><pLength>N257</pLength><AccessMode>RO</AccessMode><pPort>EventExposureStartPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <IntSwissKnife Name="N256"><pVariable Name="V1">N466</pVariable><Formula>(V1) ? 0x8 : 0x6</Formula></IntSwissKnife>
  <IntSwissKnife Name="N257"><pVariable Name="V1">N466</pVariable><Formula>(V1) ? 0x8 : 0x2</Formula></IntSwissKnife>
  <StructReg Comment="N465"><Address>0x0960</Address><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>BigEndian</Endianess><StructEntry Name="N466"><Bit>1</Bit></StructEntry></StructReg>
  <Port Name="EventExposureStartPort"><EventID>900B</EventID></Port>

  <IntReg Name="Scratch"><Address>0x200</Address><pLength>ScratchLength</pLength><AccessMode>RW</AccessMode><pPort>Device</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <Integer Name="ScratchLength"><Value>2</Value></Integer>
</RegisterDescription>
"#;

fn trt009s_nodemap() -> NodeMap {
    let model = viva_genapi_xml::parse(TRT009S_XML).expect("parse");
    let map = NodeMap::try_from_xml(model).expect("build");
    assert!(map.skipped().is_empty(), "{:?}", map.skipped());
    map
}

/// An `<IntReg>` reads as many bytes as its `<pLength>` node gives, where its
/// `<pAddress>` node puts them — on the TRT009S, 2 bytes at 0x6 for 16-bit
/// block ids, 8 at 0x8 for 64-bit ones. The default 4 bytes at 0x6 read the
/// block id and half the timestamp: 0x0102_0000.
#[test]
fn an_int_reg_reads_the_length_its_p_length_gives() {
    let mut map = trt009s_nodemap();
    let io = MemIo::default();

    // Extended ids off: the 16-bit block id 0x0102 at 0x6.
    io.regs.borrow_mut().insert(0x0960, vec![0, 0, 0, 0]);
    #[rustfmt::skip]
    let short: [u8; 16] = [
        0x00, 0x00, 0x90, 0x0B, 0x00, 0x00, 0x01, 0x02,
        0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9A,
    ];
    map.attach_event(0x900B, Bytes::copy_from_slice(&short));
    assert_eq!(
        map.get_integer("EventExposureStartFrameID", &io)
            .expect("16-bit frame id"),
        0x0102
    );

    // Extended ids on — `<Bit>1</Bit>` counts from the MSB of a big-endian
    // register: the 64-bit block id at 0x8.
    io.regs
        .borrow_mut()
        .insert(0x0960, 0x4000_0000u32.to_be_bytes().to_vec());
    map.invalidate_all();
    #[rustfmt::skip]
    let extended: [u8; 24] = [
        0x00, 0x00, 0x90, 0x0B, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
        0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9A,
    ];
    map.attach_event(0x900B, Bytes::copy_from_slice(&extended));
    assert_eq!(
        map.get_integer("EventExposureStartFrameID", &io)
            .expect("64-bit frame id"),
        0x0000_0001_0203_0405
    );
}

/// A write goes out at the length the `<pLength>` node gives too, not as the
/// default 4 bytes over whatever follows the register.
#[test]
fn an_int_reg_writes_the_length_its_p_length_gives() {
    let mut map = trt009s_nodemap();
    let io = MemIo::default();
    map.set_integer("Scratch", 0x1234, &io).expect("write");
    assert_eq!(io.writes(), vec![(0x200, vec![0x12, 0x34])]);

    io.regs.borrow_mut().insert(0x200, vec![0xAB, 0xCD]);
    map.invalidate_all();
    assert_eq!(map.get_integer("Scratch", &io).expect("read"), 0xABCD);
    assert_eq!(io.reads(), vec![(0x200, 2)]);
}
