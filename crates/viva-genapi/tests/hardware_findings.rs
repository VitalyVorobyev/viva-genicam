//! What a real USB3 Vision camera, a MATRIX VISION mvBlueFOX3-2004G, showed
//! that the fakes and the corpus had not: a `<String>` that delegates through
//! `<pValue>`, a `<Register>` whose length another node gives, an
//! enumeration entry the camera does not implement, and a selector the
//! camera does not implement in front of the feature everyone saves.
//!
//! The fixture is a miniature of the camera's own XML (`MajorVersion="71"`):
//! node names, addresses, lengths and gating are copied from it; only the
//! number of nodes is cut down. The camera's document is not in the corpus,
//! so these constructs are pinned here.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use viva_genapi::persistence::{Configuration, SavedValue};
use viva_genapi::{GenApiError, NodeMap, RegisterIo};

const XML: &str = r#"<RegisterDescription ModelName="mvBlueFOX3" VendorName="MATRIX_VISION" SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0" MajorVersion="71" MinorVersion="0" SubMinorVersion="0">
  <Port Name="Device"/>
  <!-- The camera gates its features on bits of inquiry registers, read
       little-endian; a set bit is "implemented". -->
  <StructReg Comment="Inquiry">
    <Address>0x2000</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>Device</pPort>
    <Endianess>LittleEndian</Endianess>
    <StructEntry Name="QO"><Bit>8</Bit></StructEntry>
    <StructEntry Name="OP"><Bit>9</Bit></StructEntry>
    <StructEntry Name="OW"><Bit>10</Bit></StructEntry>
    <StructEntry Name="RT"><Bit>11</Bit></StructEntry>
    <StructEntry Name="TT"><Bit>12</Bit></StructEntry>
  </StructReg>

  <!-- H-1: a <String> that takes its value from a <StringReg>. -->
  <StringReg Name="DBB"><Address>0x1000134</Address><Length>32</Length><AccessMode>RO</AccessMode><pPort>Device</pPort></StringReg>
  <String Name="DeviceFirmwareVersion" NameSpace="Standard">
    <DisplayName>Device Firmware Version</DisplayName>
    <pIsImplemented>QO</pIsImplemented>
    <pValue>DBB</pValue>
  </String>
  <!-- and a floating one, as aravis_genicam.xml's StringNodeB. -->
  <String Name="StringNodeB"><Value>Hello World!</Value></String>

  <!-- H-3: a <Register> whose length another node holds. -->
  <IntReg Name="mvDeviceFirmwareHash_InqLength"><Address>0x10000ac</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>Device</pPort><Sign>Signed</Sign><Endianess>LittleEndian</Endianess></IntReg>
  <Register Name="mvDeviceFirmwareHashCode" NameSpace="Custom">
    <pIsImplemented>OP</pIsImplemented>
    <Address>0x10001a0</Address>
    <pLength>mvDeviceFirmwareHash_InqLength</pLength>
    <AccessMode>RO</AccessMode>
    <pPort>Device</pPort>
  </Register>
  <!-- and a <StringReg>, as Basler's acA1600-20gm DeviceFirmwareVersion. -->
  <IntReg Name="DeviceFirmwareVersion_InqLengthFao"><Address>0x3100</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>
  <StringReg Name="BaslerFirmwareVersion"><Address>0x3200</Address><pLength>DeviceFirmwareVersion_InqLengthFao</pLength><AccessMode>RO</AccessMode><pPort>Device</pPort></StringReg>

  <!-- H-4: entries gated on what this model implements. -->
  <Enumeration Name="TestPattern" NameSpace="Standard">
    <Streamable>Yes</Streamable>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="GreyHorizontalRamp"><pIsImplemented>RT</pIsImplemented><Value>4</Value></EnumEntry>
    <EnumEntry Name="mvBayerRaw"><pIsImplemented>TT</pIsImplemented><Value>6</Value></EnumEntry>
    <pValue>CDB</pValue>
  </Enumeration>
  <IntReg Name="CDB"><Address>0x1020000</Address><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>

  <!-- H-5: a selector this model does not implement, in front of the
       feature every configuration holds. -->
  <Enumeration Name="ExposureTimeSelector" NameSpace="Standard">
    <pIsImplemented>OW</pIsImplemented>
    <EnumEntry Name="Common"><Value>0</Value></EnumEntry>
    <EnumEntry Name="mvHorizontalZone0"><Value>1</Value></EnumEntry>
    <pValue>XDB</pValue>
    <pSelected>ExposureTime</pSelected>
  </Enumeration>
  <IntReg Name="XDB"><Address>0x1020010</Address><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>
  <Float Name="ExposureTime" NameSpace="Standard">
    <Streamable>Yes</Streamable>
    <pValue>ZDB</pValue>
    <Min>10</Min><Max>1000000</Max>
  </Float>
  <Converter Name="ZDB"><FormulaTo>FROM * 1</FormulaTo><FormulaFrom>TO / 1</FormulaFrom><pValue>YDB</pValue></Converter>
  <IntReg Name="YDB"><Address>0x1020014</Address><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>

  <!-- H-6: a selector the camera lets us set but whose writes it refuses. -->
  <Enumeration Name="mvAreaSelector">
    <EnumEntry Name="mvArea0"><Value>0</Value></EnumEntry>
    <EnumEntry Name="mvArea1"><Value>1</Value></EnumEntry>
    <pValue>AreaSelectorReg</pValue>
    <pSelected>mvAreaWidth</pSelected>
  </Enumeration>
  <IntReg Name="AreaSelectorReg"><Address>0x1030000</Address><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>
  <Integer Name="mvAreaWidth"><Streamable>Yes</Streamable><pValue>AreaWidthReg</pValue><Min>0</Min><Max>4096</Max></Integer>
  <IntReg Name="AreaWidthReg"><Address>0x1030004</Address><pIndex Offset="4">AreaSelectorReg</pIndex><Length>4</Length><AccessMode>RW</AccessMode><pPort>Device</pPort><Endianess>LittleEndian</Endianess></IntReg>
</RegisterDescription>"#;

const INQUIRY: u64 = 0x2000;
const FIRMWARE: u64 = 0x1000134;
const HASH_LEN: u64 = 0x10000ac;
const HASH: u64 = 0x10001a0;
const BASLER_LEN: u64 = 0x3100;
const BASLER_FIRMWARE: u64 = 0x3200;
const TEST_PATTERN: u64 = 0x1020000;
const EXPOSURE_SELECTOR: u64 = 0x1020010;
const EXPOSURE: u64 = 0x1020014;
const AREA_SELECTOR: u64 = 0x1030000;
const AREA_WIDTH: u64 = 0x1030004;

/// Bits of the inquiry register: what this camera reports implemented.
/// `DeviceFirmwareVersion` (8), `mvDeviceFirmwareHashCode` (9) and
/// `mvBayerRaw` (12) are; `ExposureTimeSelector` (10) and
/// `GreyHorizontalRamp` (11) are not.
const IMPLEMENTED: u32 = (1 << 8) | (1 << 9) | (1 << 12);

/// Byte-addressed memory with the camera's contents, which can refuse the
/// writes to some addresses as a device answers `ACCESS_DENIED`.
#[derive(Default)]
struct Camera {
    bytes: RefCell<HashMap<u64, u8>>,
    /// Every write asked for, refused or not.
    writes: RefCell<Vec<u64>>,
    refuses: HashSet<u64>,
}

impl Camera {
    fn new() -> Self {
        let camera = Camera::default();
        camera.put_le(INQUIRY, IMPLEMENTED);
        camera.put_bytes(FIRMWARE, b"2.32.1676.0\0");
        camera.put_le(HASH_LEN, 20);
        camera.put_bytes(HASH, &(1..=40).collect::<Vec<u8>>());
        camera.put_le(BASLER_LEN, 8);
        camera.put_bytes(BASLER_FIRMWARE, b"V3.9-0-1234567890");
        camera.put_le(EXPOSURE, 20000);
        camera.put_le(AREA_WIDTH, 640);
        camera.put_le(AREA_WIDTH + 4, 320);
        camera
    }

    fn refusing(mut self, addr: u64) -> Self {
        self.refuses.insert(addr);
        self
    }

    fn put_le(&self, addr: u64, value: u32) {
        self.put_bytes(addr, &value.to_le_bytes());
    }

    fn put_bytes(&self, addr: u64, data: &[u8]) {
        let mut bytes = self.bytes.borrow_mut();
        for (i, b) in data.iter().enumerate() {
            bytes.insert(addr + i as u64, *b);
        }
    }

    fn le(&self, addr: u64) -> u32 {
        let bytes = self.bytes.borrow();
        u32::from_le_bytes(std::array::from_fn(|i| {
            bytes.get(&(addr + i as u64)).copied().unwrap_or(0)
        }))
    }

    fn writes_to(&self, addr: u64) -> usize {
        self.writes.borrow().iter().filter(|a| **a == addr).count()
    }
}

impl RegisterIo for Camera {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let bytes = self.bytes.borrow();
        Ok((0..len as u64)
            .map(|i| bytes.get(&(addr + i)).copied().unwrap_or(0))
            .collect())
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.writes.borrow_mut().push(addr);
        if self.refuses.contains(&addr) {
            return Err(GenApiError::Io("ACCESS_DENIED".into()));
        }
        self.put_bytes(addr, data);
        Ok(())
    }
}

fn nodemap() -> NodeMap {
    NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("build")
}

// --- H-1: <String> with <pValue> or <Value> ---------------------------------

/// GenICam Standard v2.1.1 §2.8.12 and the GenApi schema: a `<String>` holds
/// a `<Value>` or takes it from the node `<pValue>` names. The camera's
/// `DeviceFirmwareVersion` read as an empty string, although its `<StringReg>`
/// held the version.
#[test]
fn a_string_reads_through_its_p_value() {
    let nm = nodemap();
    let camera = Camera::new();
    assert_eq!(
        nm.get_string("DeviceFirmwareVersion", &camera).unwrap(),
        "2.32.1676.0"
    );
}

/// The access of a delegating `<String>` is its `<pValue>`'s: the register
/// is read-only, so the string is too.
#[test]
fn a_string_is_as_writable_as_its_p_value() {
    let mut nm = nodemap();
    let camera = Camera::new();
    assert_eq!(
        nm.declared_access_mode("DeviceFirmwareVersion"),
        Some(viva_genapi::AccessMode::RO)
    );
    let err = nm
        .set_string("DeviceFirmwareVersion", "9.9", &camera)
        .expect_err("a read-only register refuses the write");
    assert!(
        matches!(err, GenApiError::Access { .. }),
        "refused locally, by access mode: {err}"
    );
    assert_eq!(camera.writes_to(FIRMWARE), 0);
}

/// A floating `<String>` starts at its `<Value>` and holds what is written,
/// as a floating `<Integer>` does; nothing reaches the device.
#[test]
fn a_floating_string_holds_its_value() {
    let mut nm = nodemap();
    let camera = Camera::new();
    assert_eq!(
        nm.get_string("StringNodeB", &camera).unwrap(),
        "Hello World!"
    );
    nm.set_string("StringNodeB", "Goodbye", &camera).unwrap();
    assert_eq!(nm.get_string("StringNodeB", &camera).unwrap(), "Goodbye");
    assert!(camera.writes.borrow().is_empty());
}

// --- H-3: <pLength> ----------------------------------------------------------

/// GenICam Standard v2.1.1 §2.8.3: "Alternatively the length can be read
/// from another node using an <pLength> entry." The camera's
/// `mvDeviceFirmwareHashCode` was skipped for it.
#[test]
fn a_register_reads_the_length_its_p_length_names() {
    let nm = nodemap();
    let camera = Camera::new();
    let hash = nm
        .get_register("mvDeviceFirmwareHashCode", &camera)
        .expect("the register is built and read");
    assert_eq!(hash, (1..=20).collect::<Vec<u8>>());
}

/// The same element on a `<StringReg>`: Basler's `DeviceFirmwareVersion`
/// and FLIR's `ChunkSerialData` declare their length this way.
#[test]
fn a_string_reg_reads_the_length_its_p_length_names() {
    let nm = nodemap();
    let camera = Camera::new();
    assert_eq!(
        nm.get_string("BaslerFirmwareVersion", &camera).unwrap(),
        "V3.9-0-1"
    );
}

// --- H-4: entries not available now ---------------------------------------

/// GenApi refuses to set an enumeration to an entry that is not available —
/// its `pIsImplemented` or `pIsAvailable` reads false. The camera accepted
/// `TestPattern=GreyHorizontalRamp` from us and streamed zeros.
#[test]
fn an_entry_that_is_not_implemented_is_refused_and_nothing_is_written() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let err = nm
        .set_enum("TestPattern", "GreyHorizontalRamp", &camera)
        .expect_err("GreyHorizontalRamp is not implemented on this model");
    assert!(
        matches!(
            &err,
            GenApiError::EnumEntryUnavailable { node, entry }
                if node == "TestPattern" && entry == "GreyHorizontalRamp"
        ),
        "a typed refusal naming the entry: {err:?}"
    );
    assert_eq!(camera.writes_to(TEST_PATTERN), 0);
    // An implemented entry is written as before.
    nm.set_enum("TestPattern", "mvBayerRaw", &camera).unwrap();
    assert_eq!(camera.le(TEST_PATTERN), 6);
}

/// Text goes through the same check.
#[test]
fn text_naming_an_entry_that_is_not_available_is_refused() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let err = nm
        .set_text("TestPattern", "GreyHorizontalRamp", &camera)
        .expect_err("refused");
    assert!(
        matches!(err, GenApiError::EnumEntryUnavailable { .. }),
        "{err:?}"
    );
}

/// A configuration that holds such an entry loads everything else, and
/// reports the one it could not write with the reason.
#[test]
fn a_load_reports_an_entry_that_is_not_available_and_writes_the_rest() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let configuration = Configuration {
        identity: nm.identity().clone(),
        values: vec![
            saved("TestPattern", "GreyHorizontalRamp"),
            saved("ExposureTime", "30"),
        ],
        omitted: Vec::new(),
    };
    let report = nm.load_configuration(&configuration, &camera);
    assert_eq!(report.written, 1);
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert_eq!(report.failures[0].value.feature, "TestPattern");
    assert!(matches!(
        report.failures[0].error,
        GenApiError::EnumEntryUnavailable { .. }
    ));
    assert_eq!(camera.writes_to(TEST_PATTERN), 0);
    assert_eq!(camera.le(EXPOSURE), 30);
}

// --- H-5: a selector that cannot select now ---------------------------------

/// A selector the camera does not implement, or does not let us set now,
/// selects nothing: the feature has one value, and that is saved, once,
/// without the selector. Before, `ExposureTime` was left out of every save
/// ("its selector cannot be set to Common: node unavailable").
#[test]
fn a_feature_behind_a_selector_that_cannot_select_is_saved_once() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let saved = nm.save_configuration(&camera);
    let exposure: Vec<&SavedValue> = saved
        .values
        .iter()
        .filter(|v| v.feature == "ExposureTime")
        .collect();
    assert_eq!(exposure.len(), 1, "saved: {:?}", saved.values);
    assert!(exposure[0].selectors.is_empty(), "{:?}", exposure[0]);
    assert_eq!(exposure[0].value, "20000");
    assert!(
        !saved.omitted.iter().any(|o| o.feature == "ExposureTime"),
        "{:?}",
        saved.omitted
    );
    // The selector was neither set nor put back.
    assert_eq!(camera.writes_to(EXPOSURE_SELECTOR), 0);
}

/// ... and a load writes it back.
#[test]
fn a_saved_exposure_time_is_restored() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let saved = nm.save_configuration(&camera);
    let text = saved.to_text();
    assert!(text.contains("ExposureTime\t20000\n"), "{text}");

    nm.set_float("ExposureTime", 30.0, &camera).unwrap();
    assert_eq!(camera.le(EXPOSURE), 30);
    let report = nm.load_configuration(&Configuration::parse(&text).unwrap(), &camera);
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!(camera.le(EXPOSURE), 20000);
}

// --- H-6: put a selector back only if it moved ------------------------------

/// A selector whose every write the device refused never moved, so there is
/// nothing to put back — and no warning to print about it.
#[test]
fn a_selector_that_never_moved_is_not_put_back() {
    let mut nm = nodemap();
    let camera = Camera::new().refusing(AREA_SELECTOR);
    let saved = nm.save_configuration(&camera);
    // One attempt per entry; none to put it back.
    assert_eq!(camera.writes_to(AREA_SELECTOR), 2);
    assert!(
        saved
            .omitted
            .iter()
            .filter(|o| o.feature == "mvAreaWidth")
            .count()
            == 2,
        "{:?}",
        saved.omitted
    );
}

/// A load does not move a selector that already holds the entry a value was
/// saved under, so it has nothing to put back either.
#[test]
fn a_load_leaves_a_selector_alone_that_already_holds_the_entry() {
    let mut nm = nodemap();
    let camera = Camera::new();
    let configuration = Configuration {
        identity: nm.identity().clone(),
        values: vec![SavedValue {
            feature: "mvAreaWidth".into(),
            selectors: vec![("mvAreaSelector".into(), "mvArea0".into())],
            value: "512".into(),
        }],
        omitted: Vec::new(),
    };
    let report = nm.load_configuration(&configuration, &camera);
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!(camera.le(AREA_WIDTH), 512);
    assert_eq!(camera.writes_to(AREA_SELECTOR), 0);
}

fn saved(feature: &str, value: &str) -> SavedValue {
    SavedValue {
        feature: feature.into(),
        selectors: Vec::new(),
        value: value.into(),
    }
}
