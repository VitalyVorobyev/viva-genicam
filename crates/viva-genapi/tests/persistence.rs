//! GA-14: saving the `<Streamable>` features and restoring them, against an
//! in-memory device that behaves as cameras do — a binning change resets the
//! width — so that the order of writes is observable in the device's state.

use std::cell::RefCell;
use std::collections::HashMap;

use viva_genapi::persistence::{Configuration, SavedValue};
use viva_genapi::{DocumentIdentity, GenApiError, NodeMap, RegisterIo};

/// Every streamable shape the corpus uses, and the dependencies that make the
/// order of a restore matter. Declaration order is deliberately the wrong
/// order to write in: `Width` before the `BinningHorizontal` that resets it,
/// `ExposureTime` before the `ExposureAuto` that locks it.
const XML: &str = r#"<RegisterDescription ModelName="Test-1" VendorName="Acme" SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0" MajorVersion="2" MinorVersion="0" SubMinorVersion="3">
  <Category Name="Root"><pFeature>Width</pFeature></Category>
  <Integer Name="SensorWidth"><Address>0x100</Address><Length>4</Length><AccessMode>RO</AccessMode></Integer>
  <Integer Name="Width">
    <Streamable>Yes</Streamable>
    <Address>0x0</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Min>1</Min><pMax>WidthMaxNow</pMax>
    <pInvalidator>BinningHorizontal</pInvalidator>
  </Integer>
  <IntSwissKnife Name="WidthMaxNow">
    <pVariable Name="S">SensorWidth</pVariable><pVariable Name="B">BinningHorizontal</pVariable><pVariable Name="X">OffsetX</pVariable>
    <Formula>S / B - X</Formula>
  </IntSwissKnife>
  <Integer Name="OffsetX">
    <Streamable>Yes</Streamable>
    <Address>0x4</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Min>0</Min><pMax>OffsetXMax</pMax>
  </Integer>
  <IntSwissKnife Name="OffsetXMax">
    <pVariable Name="S">SensorWidth</pVariable><pVariable Name="B">BinningHorizontal</pVariable><pVariable Name="W">Width</pVariable>
    <Formula>S / B - W</Formula>
  </IntSwissKnife>
  <Integer Name="BinningHorizontal">
    <Streamable>Yes</Streamable>
    <Address>0x8</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Min>1</Min><Max>4</Max>
  </Integer>
  <Enumeration Name="GainSelector">
    <Streamable>Yes</Streamable>
    <EnumEntry Name="AnalogAll"><Value>0</Value></EnumEntry>
    <EnumEntry Name="DigitalAll"><Value>1</Value></EnumEntry>
    <pValue>GainSelectorReg</pValue>
    <pSelected>Gain</pSelected>
  </Enumeration>
  <IntReg Name="GainSelectorReg"><Address>0x10</Address><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Integer Name="Gain">
    <Streamable>Yes</Streamable>
    <pValue>GainReg</pValue><Min>0</Min><Max>100</Max>
  </Integer>
  <IntReg Name="GainReg"><Address>0x20</Address><pIndex Offset="4">GainSelectorReg</pIndex><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Integer Name="LUTIndex">
    <Address>0x30</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Min>0</Min><Max>3</Max>
    <pSelected>LUTValue</pSelected>
  </Integer>
  <Integer Name="LUTValue">
    <Streamable>Yes</Streamable>
    <pValue>LUTValueReg</pValue><Min>0</Min><Max>4095</Max>
  </Integer>
  <IntReg Name="LUTValueReg"><Address>0x40</Address><pIndex Offset="4">LUTIndex</pIndex><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Float Name="ExposureTime">
    <Streamable>Yes</Streamable>
    <pIsLocked>ExposureAutoActive</pIsLocked>
    <pValue>ExposureTimeReg</pValue><Min>10</Min><Max>1000000</Max>
  </Float>
  <FloatReg Name="ExposureTimeReg"><Address>0x54</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess></FloatReg>
  <IntSwissKnife Name="ExposureAutoActive"><pVariable Name="E">ExposureAutoReg</pVariable><Formula>E</Formula></IntSwissKnife>
  <Enumeration Name="ExposureAuto">
    <Streamable>Yes</Streamable>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Continuous"><Value>2</Value></EnumEntry>
    <pValue>ExposureAutoReg</pValue>
  </Enumeration>
  <IntReg Name="ExposureAutoReg"><Address>0x50</Address><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Boolean Name="ReverseX"><Streamable>Yes</Streamable><pValue>ReverseXReg</pValue></Boolean>
  <IntReg Name="ReverseXReg"><Address>0x70</Address><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Register Name="LUTValueAll"><Streamable>Yes</Streamable><Address>0x80</Address><Length>4</Length><AccessMode>RW</AccessMode></Register>
  <StringReg Name="DeviceUserID"><Streamable>Yes</Streamable><Address>0x60</Address><Length>8</Length><AccessMode>RW</AccessMode></StringReg>
  <StringReg Name="DeviceVendorName"><Streamable>Yes</Streamable><Address>0x90</Address><Length>8</Length><AccessMode>RO</AccessMode></StringReg>
  <IntSwissKnife Name="Never"><Formula>0</Formula></IntSwissKnife>
  <Integer Name="Unbuilt"><Streamable>Yes</Streamable><pIsImplemented>Never</pIsImplemented><Address>0xA0</Address><Length>4</Length><AccessMode>RW</AccessMode></Integer>
  <Integer Name="Temperature"><Address>0xB0</Address><Length>4</Length><AccessMode>RO</AccessMode></Integer>
  <Command Name="AcquisitionStart"><Address>0xC0</Address><Length>4</Length><CommandValue>1</CommandValue></Command>
</RegisterDescription>"#;

const WIDTH: u64 = 0x0;
const OFFSET_X: u64 = 0x4;
const BINNING: u64 = 0x8;
const GAIN_SELECTOR: u64 = 0x10;
const GAIN: u64 = 0x20;
const LUT_INDEX: u64 = 0x30;
const LUT_VALUE: u64 = 0x40;
const EXPOSURE_AUTO: u64 = 0x50;
const EXPOSURE_TIME: u64 = 0x54;
const USER_ID: u64 = 0x60;
const REVERSE_X: u64 = 0x70;
const LUT_ALL: u64 = 0x80;
const SENSOR_WIDTH: u64 = 0x100;
const SENSOR_PIXELS: u32 = 1280;

/// Byte-addressed big-endian memory. Writing the binning register resets the
/// width to the new maximum, as cameras do; nothing else has a side effect.
#[derive(Default)]
struct Device {
    bytes: RefCell<HashMap<u64, u8>>,
    writes: RefCell<Vec<u64>>,
}

impl Device {
    fn new() -> Self {
        let device = Device::default();
        device.put(SENSOR_WIDTH, SENSOR_PIXELS);
        device.put(WIDTH, SENSOR_PIXELS);
        device.put(BINNING, 1);
        device.put(EXPOSURE_TIME, 5000f32.to_bits());
        device
    }

    fn put(&self, addr: u64, value: u32) {
        let mut bytes = self.bytes.borrow_mut();
        for (i, b) in value.to_be_bytes().into_iter().enumerate() {
            bytes.insert(addr + i as u64, b);
        }
    }

    fn word(&self, addr: u64) -> u32 {
        let bytes = self.bytes.borrow();
        u32::from_be_bytes(std::array::from_fn(|i| {
            bytes.get(&(addr + i as u64)).copied().unwrap_or(0)
        }))
    }
}

impl RegisterIo for Device {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let bytes = self.bytes.borrow();
        Ok((0..len as u64)
            .map(|i| bytes.get(&(addr + i)).copied().unwrap_or(0))
            .collect())
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        {
            let mut bytes = self.bytes.borrow_mut();
            for (i, b) in data.iter().enumerate() {
                bytes.insert(addr + i as u64, *b);
            }
        }
        self.writes.borrow_mut().push(addr);
        if addr == BINNING {
            let binning = self.word(BINNING).max(1);
            self.put(WIDTH, SENSOR_PIXELS / binning - self.word(OFFSET_X));
        }
        Ok(())
    }
}

fn nodemap() -> NodeMap {
    NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("build")
}

fn value(feature: &str, selectors: &[(&str, &str)], value: &str) -> SavedValue {
    SavedValue {
        feature: feature.into(),
        selectors: selectors
            .iter()
            .map(|(s, e)| (s.to_string(), e.to_string()))
            .collect(),
        value: value.into(),
    }
}

fn configuration(values: Vec<SavedValue>) -> Configuration {
    Configuration {
        identity: nodemap().identity().clone(),
        values,
        omitted: Vec::new(),
    }
}

#[test]
fn the_plan_holds_the_streamable_features_and_writes_providers_first() {
    let plan = nodemap().persistence_plan();
    let names: Vec<&str> = plan.features().iter().map(|f| f.name.as_str()).collect();
    for expected in [
        "Width",
        "OffsetX",
        "BinningHorizontal",
        "GainSelector",
        "Gain",
        "LUTValue",
        "ExposureTime",
        "ExposureAuto",
        "ReverseX",
        "LUTValueAll",
        "DeviceUserID",
        "DeviceVendorName",
        "Unbuilt",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    for absent in ["LUTIndex", "Temperature", "AcquisitionStart", "Root"] {
        assert!(!names.contains(&absent), "{absent} is not streamable");
    }
    let before = |a: &str, b: &str| plan.position(a).unwrap() < plan.position(b).unwrap();
    assert!(before("BinningHorizontal", "Width"), "{names:?}");
    assert!(before("BinningHorizontal", "OffsetX"), "{names:?}");
    assert!(before("GainSelector", "Gain"), "{names:?}");
    assert!(before("ExposureAuto", "ExposureTime"), "{names:?}");
    // Width and OffsetX bound each other: document order between them.
    assert!(before("Width", "OffsetX"), "{names:?}");

    let gain = &plan.features()[plan.position("Gain").unwrap()];
    assert_eq!(gain.selectors, ["GainSelector"]);
    assert!(gain.after.contains(&"GainSelector".to_string()));
    let lut = &plan.features()[plan.position("LUTValue").unwrap()];
    assert_eq!(
        lut.selectors,
        ["LUTIndex"],
        "a selector need not be streamable"
    );
}

#[test]
fn a_save_reads_every_selector_entry_and_puts_each_selector_back() {
    let device = Device::new();
    device.put(GAIN_SELECTOR, 1);
    device.put(GAIN, 7);
    device.put(GAIN + 4, 9);
    device.put(LUT_INDEX, 2);
    for i in 0..4 {
        device.put(LUT_VALUE + 4 * i, 100 + i as u32);
    }
    let mut map = nodemap();
    let saved = map.save_configuration(&device);

    let gains: Vec<&SavedValue> = saved
        .values
        .iter()
        .filter(|v| v.feature == "Gain")
        .collect();
    assert_eq!(
        gains,
        [
            &value("Gain", &[("GainSelector", "AnalogAll")], "7"),
            &value("Gain", &[("GainSelector", "DigitalAll")], "9"),
        ]
    );
    let luts: Vec<String> = saved
        .values
        .iter()
        .filter(|v| v.feature == "LUTValue")
        .map(|v| format!("{}={}", v.selectors[0].1, v.value))
        .collect();
    assert_eq!(luts, ["0=100", "1=101", "2=102", "3=103"]);
    assert!(
        saved
            .values
            .contains(&value("GainSelector", &[], "DigitalAll"))
    );
    assert!(saved.values.contains(&value("ReverseX", &[], "0")));
    assert!(
        saved
            .values
            .contains(&value("LUTValueAll", &[], "0x00000000"))
    );
    assert!(saved.values.contains(&value("ExposureTime", &[], "5000")));

    assert_eq!(device.word(GAIN_SELECTOR), 1, "GainSelector put back");
    assert_eq!(device.word(LUT_INDEX), 2, "LUTIndex put back");
    assert_eq!(saved.identity.model_name, "Test-1");
}

#[test]
fn a_save_says_why_it_left_a_feature_out() {
    let device = Device::new();
    device.put(EXPOSURE_AUTO, 2);
    let mut map = nodemap();
    let saved = map.save_configuration(&device);
    let reason = |feature: &str| {
        saved
            .omitted
            .iter()
            .find(|o| o.feature == feature)
            .map(|o| o.reason.clone())
            .unwrap_or_else(|| panic!("{feature} not omitted: {:?}", saved.omitted))
    };
    assert_eq!(reason("DeviceVendorName"), "read-only");
    assert_eq!(reason("Unbuilt"), "not implemented");
    assert_eq!(reason("ExposureTime"), "locked by ExposureAutoActive");
    assert_eq!(
        reason("DeviceUserID"),
        "empty, and the file has no way to hold an empty value"
    );
    assert!(!saved.values.iter().any(|v| v.feature == "ExposureTime"));
    assert!(
        saved
            .values
            .contains(&value("ExposureAuto", &[], "Continuous"))
    );
}

/// The layout, byte for byte, from the reference implementation's writer
/// (`Persistence.h`, `operator<<(ostream, CFeatureBag)`): the magic line,
/// comments, `Name<TAB>Value`, and `Name<TAB>{Sel=Entry}<TAB>Value`.
#[test]
fn a_configuration_is_written_in_the_genapi_feature_bag_layout() {
    let configuration = Configuration {
        identity: DocumentIdentity {
            vendor_name: "Acme".into(),
            model_name: "Test-1".into(),
            version: "2.0.3".into(),
        },
        values: vec![
            value("GainSelector", &[], "AnalogAll"),
            value("Gain", &[("GainSelector", "DigitalAll")], "9"),
            value(
                "LUTValue",
                &[("LUTSelector", "Luminance"), ("LUTIndex", "3")],
                "103",
            ),
        ],
        omitted: vec![viva_genapi::persistence::Omission {
            feature: "ExposureTime".into(),
            selectors: Vec::new(),
            reason: "locked by ExposureAuto".into(),
        }],
    };
    let text = configuration.to_text();
    assert_eq!(
        text,
        "# {05D8C294-F295-4dfb-9D01-096BD04049F4}\n\
         # viva-genicam configuration\n\
         # VendorName = Acme\n\
         # ModelName = Test-1\n\
         # DeviceXmlVersion = 2.0.3\n\
         # not saved: ExposureTime: locked by ExposureAuto\n\
         GainSelector\tAnalogAll\n\
         Gain\t{GainSelector=DigitalAll}\t9\n\
         LUTValue\t{LUTSelector=Luminance\tLUTIndex=3}\t103\n"
    );
    let parsed = Configuration::parse(&text).expect("parse");
    assert_eq!(parsed.identity, configuration.identity);
    assert_eq!(parsed.values, configuration.values);
}

#[test]
fn a_file_is_read_with_carriage_returns_comments_and_another_writers_header() {
    let text = "# {05D8C294-F295-4dfb-9D01-096BD04049F4}\r\n\
                # GenApi persistence file (version 3.1.0)\r\n\
                # Device = something else\r\n\
                \r\n\
                Width\t640\r\n\
                Gain\t{GainSelector=DigitalAll}\t5\r\n\
                DeviceUserID\tcam one\r\n";
    let parsed = Configuration::parse(text).expect("parse");
    assert_eq!(
        parsed.values,
        [
            value("Width", &[], "640"),
            value("Gain", &[("GainSelector", "DigitalAll")], "5"),
            value("DeviceUserID", &[], "cam one"),
        ]
    );
    assert_eq!(parsed.identity, DocumentIdentity::default());

    let bagger = "# {4709CB3C-7322-4460-84C3-DA11DDA09939}\nWidth\t640\n";
    assert!(
        Configuration::parse(bagger).is_err(),
        "CFeatureBagger files"
    );
    let err = Configuration::parse("Width\t640\nHeight\n").unwrap_err();
    assert_eq!(err.line, 2, "{err}");
    let err = Configuration::parse("Gain\t{GainSelector}\t5\n").unwrap_err();
    assert_eq!(err.line, 1, "{err}");
}

#[test]
fn a_save_reads_back_into_the_same_device_state() {
    let device = Device::new();
    device.put(GAIN, 7);
    device.put(GAIN + 4, 9);
    device.put(LUT_ALL, 0xA1B2_C3D4);
    device.put(REVERSE_X, 1);
    let mut map = nodemap();
    let saved = map.save_configuration(&device);
    let text = saved.to_text();

    let other = Device::new();
    let mut map = nodemap();
    let report = map.load_configuration(&Configuration::parse(&text).unwrap(), &other);
    assert!(report.is_complete(), "{:?}", report.failures);
    for addr in [GAIN, GAIN + 4, LUT_ALL, REVERSE_X, WIDTH, EXPOSURE_TIME] {
        assert_eq!(other.word(addr), device.word(addr), "register {addr:#x}");
    }
}

/// The file lists `Width` before the `BinningHorizontal` that resets it. A
/// load in file order leaves the device at the reset width; the plan's order
/// writes the binning first.
#[test]
fn a_load_writes_in_dependency_order_whatever_the_file_order() {
    let device = Device::new();
    device.put(EXPOSURE_AUTO, 2);
    let mut map = nodemap();
    let report = map.load_configuration(
        &configuration(vec![
            value("Width", &[], "320"),
            value("ExposureTime", &[], "800"),
            value("BinningHorizontal", &[], "2"),
            value("ExposureAuto", &[], "Off"),
        ]),
        &device,
    );
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!(device.word(BINNING), 2);
    assert_eq!(
        device.word(WIDTH),
        320,
        "Width written after the binning reset it"
    );
    assert_eq!(f32::from_bits(device.word(EXPOSURE_TIME)), 800.0);
    assert_eq!(
        report.written_on_retry, 0,
        "ExposureAuto unlocked ExposureTime before it was written"
    );
}

/// `Width` and `OffsetX` bound each other. Moving the region from the right
/// edge to the full sensor needs `OffsetX` first; the plan writes `Width`
/// first, its write fails, and the second pass succeeds.
#[test]
fn a_load_retries_once_what_failed_because_of_the_order() {
    let device = Device::new();
    device.put(WIDTH, 640);
    device.put(OFFSET_X, 600);
    let mut map = nodemap();
    let report = map.load_configuration(
        &configuration(vec![
            value("Width", &[], "1280"),
            value("OffsetX", &[], "0"),
        ]),
        &device,
    );
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!((report.written, report.written_on_retry), (2, 1));
    assert_eq!((device.word(WIDTH), device.word(OFFSET_X)), (1280, 0));
}

#[test]
fn a_load_writes_each_selected_value_under_its_entry_and_puts_the_selector_back() {
    let device = Device::new();
    let mut map = nodemap();
    let report = map.load_configuration(
        &configuration(vec![
            value("Gain", &[("GainSelector", "AnalogAll")], "3"),
            value("Gain", &[("GainSelector", "DigitalAll")], "5"),
            value("GainSelector", &[], "DigitalAll"),
            value("LUTValue", &[("LUTIndex", "1")], "11"),
        ]),
        &device,
    );
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!((device.word(GAIN), device.word(GAIN + 4)), (3, 5));
    assert_eq!(device.word(GAIN_SELECTOR), 1, "the file's own GainSelector");
    assert_eq!(device.word(LUT_VALUE + 4), 11);
    assert_eq!(device.word(LUT_INDEX), 0, "LUTIndex put back");
}

/// A file that sets a selector on one line and its feature on the next, as
/// a replay of writes, binds each value to the selector line before it.
#[test]
fn a_file_of_plain_lines_binds_each_value_to_the_selector_line_before_it() {
    let device = Device::new();
    let parsed = Configuration::parse(
        "GainSelector\tAnalogAll\nGain\t3\nGainSelector\tDigitalAll\nGain\t5\n",
    )
    .unwrap();
    let mut map = nodemap();
    let report = map.load_configuration(&parsed, &device);
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!((device.word(GAIN), device.word(GAIN + 4)), (3, 5));
    assert_eq!(device.word(GAIN_SELECTOR), 1);
}

#[test]
fn a_load_reports_every_value_it_could_not_write_and_writes_the_rest() {
    let device = Device::new();
    let mut map = nodemap();
    let report = map.load_configuration(
        &configuration(vec![
            value("NoSuchFeature", &[], "1"),
            value("Gain", &[("GainSelector", "AnalogAll")], "many"),
            value("Gain", &[("GainSelector", "Neither")], "4"),
            value("DeviceVendorName", &[], "x"),
            value("OffsetX", &[], "5000"),
            value("DeviceUserID", &[], "cam one"),
        ]),
        &device,
    );
    let failed: Vec<&str> = report
        .failures
        .iter()
        .map(|f| f.value.feature.as_str())
        .collect();
    assert_eq!(
        failed,
        [
            "NoSuchFeature",
            "Gain",
            "Gain",
            "DeviceVendorName",
            "OffsetX"
        ],
        "{:?}",
        report.failures
    );
    assert!(matches!(
        report.failures[1].error,
        GenApiError::InvalidText { .. }
    ));
    assert!(matches!(report.failures[4].error, GenApiError::Range(_)));
    assert_eq!(report.written, 1);
    let user_id: Vec<u8> = device.read(USER_ID, 7).unwrap();
    assert_eq!(user_id, b"cam one");
}

#[test]
fn a_load_onto_another_model_or_xml_version_warns() {
    let device = Device::new();
    let mut map = nodemap();
    let mut other = configuration(vec![value("ReverseX", &[], "1")]);
    other.identity.model_name = "Test-2".into();
    let report = map.load_configuration(&other, &device);
    assert!(report.is_complete());
    assert_eq!(device.word(REVERSE_X), 1, "the load still runs");
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("Test-2"),
        "{:?}",
        report.warnings
    );
    assert!(
        report.warnings[0].contains("Test-1"),
        "{:?}",
        report.warnings
    );

    let mut newer = configuration(vec![]);
    newer.identity.version = "3.0.0".into();
    let report = map.load_configuration(&newer, &device);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("3.0.0"),
        "{:?}",
        report.warnings
    );

    let same = configuration(vec![]);
    assert!(map.load_configuration(&same, &device).warnings.is_empty());
}

/// Selectors of selectors: `LUTSelector` selects `LUTIndex`, which selects
/// `LUTValue`; `LUTSelector` does not name `LUTValue` itself, yet the value
/// read depends on both (the register is indexed by both). The save must walk
/// every entry of the outer selector, and under each the inner one — the
/// shape of the corpus's LUT and sequencer selectors (Codex review of #200).
const CHAINED: &str = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Enumeration Name="LUTSelector">
    <EnumEntry Name="Luminance"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Red"><Value>1</Value></EnumEntry>
    <pValue>LUTSelectorReg</pValue>
    <pSelected>LUTIndex</pSelected>
  </Enumeration>
  <IntReg Name="LUTSelectorReg"><Address>0x0</Address><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Integer Name="LUTIndex">
    <pValue>LUTIndexReg</pValue><Min>0</Min><Max>1</Max>
    <pSelected>LUTValue</pSelected>
  </Integer>
  <IntReg Name="LUTIndexReg"><Address>0x4</Address><Length>4</Length><AccessMode>RW</AccessMode></IntReg>
  <Integer Name="LUTValue">
    <Streamable>Yes</Streamable>
    <pValue>LUTValueReg</pValue><Min>0</Min><Max>4095</Max>
  </Integer>
  <IntReg Name="LUTValueReg">
    <Address>0x100</Address>
    <pIndex Offset="16">LUTSelectorReg</pIndex>
    <pIndex Offset="4">LUTIndexReg</pIndex>
    <Length>4</Length><AccessMode>RW</AccessMode>
  </IntReg>
</RegisterDescription>"#;

#[test]
fn a_save_walks_the_selectors_of_selectors_outermost_first() {
    let device = Device::default();
    for selector in 0..2u32 {
        for index in 0..2u32 {
            let addr = 0x100 + 16 * u64::from(selector) + 4 * u64::from(index);
            device.put(addr, 1000 + 10 * selector + index);
        }
    }
    device.put(0x0, 1); // Red selected
    device.put(0x4, 0);
    let mut map = NodeMap::try_from_xml(viva_genapi_xml::parse(CHAINED).unwrap()).unwrap();
    let saved = map.save_configuration(&device);
    let mut values: Vec<&SavedValue> = saved
        .values
        .iter()
        .filter(|v| v.feature == "LUTValue")
        .collect();
    values.sort_by(|a, b| a.selectors.cmp(&b.selectors));
    let expected: Vec<SavedValue> = [("Luminance", 0u32), ("Red", 1)]
        .into_iter()
        .flat_map(|(entry, selector)| {
            (0..2u32).map(move |index| {
                let addr = 0x100 + 16 * u64::from(selector) + 4 * u64::from(index);
                (entry, index, addr)
            })
        })
        .map(|(entry, index, addr)| {
            value(
                "LUTValue",
                &[("LUTSelector", entry), ("LUTIndex", &index.to_string())],
                &device.word(addr).to_string(),
            )
        })
        .collect();
    assert_eq!(values, expected.iter().collect::<Vec<_>>());
    assert_eq!(device.word(0x0), 1, "LUTSelector put back");
    assert_eq!(device.word(0x4), 0, "LUTIndex put back");

    // And a restore writes each value under both selectors.
    let other = Device::default();
    let mut map = NodeMap::try_from_xml(viva_genapi_xml::parse(CHAINED).unwrap()).unwrap();
    let report = map.load_configuration(&saved, &other);
    assert!(report.is_complete(), "{:?}", report.failures);
    for addr in [0x100, 0x104, 0x110, 0x114] {
        assert_eq!(other.word(addr), device.word(addr), "register {addr:#x}");
    }
}
