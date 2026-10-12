//! `<Cachable>`, `<PollingTime>` and the refresh API (GA-05, GA-40, ADR-0029).
//!
//! GenICam Standard v2.1.1 §2.8.3: "WriteThrough means that a value written to
//! the camera is written to the cache as well. WriteAround means that only read
//! values are written to the cache"; `NoCache` is never answered from one; and
//! `<PollingTime>` is "a recommended time interval [in ms] after which a node
//! should be invalidated".
//!
//! The device here is memory the test changes behind the node map's back, as a
//! camera changes a temperature or an auto-exposure value on its own, and it
//! rounds what some registers are written, as a camera rounds a gain. Every
//! assertion about freshness compares a read with the device's own state.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use viva_genapi::{CachingMode, GenApiError, ManualClock, NodeMap, RegisterIo};

/// Cut-down copies of corpus shapes:
///
/// - `DeviceTemperature`: FLIR BFS (`FLIR_BFS_PGE_31S4C_C.xml`) — a `<Float>`
///   over a `<Converter>` over an `<IntReg>` declared `NoCache` with a
///   `<PollingTime>` of 500 ms. FLIR's converter divides by `2**FRACBITS`
///   read from a second register; a constant 256 here.
/// - `ExposureTime`: PGR Blackfly S (`PGR_BlackflyS_13Y3M.xml`) — the same
///   chain over a `WriteAround` register polled every 500 ms.
/// - `Width`: a plain `<IntReg>`, `WriteThrough` by default, never polled —
///   the control.
/// - `Gain`: a `<FloatReg>` without `<Cachable>`; the standard's default for
///   one is `WriteAround` (§3.2).
/// - `LineOut`: one bit of a `NoCache` register whose other bits the device
///   drives, written by read-modify-write (Point Grey's GPIO registers,
///   `PGR_Flea3.xml`).
/// - `UserSetLoad`: a self-clearing command with a `<PollingTime>`, the shape
///   Sony's `CVC_EH6300` declares (whose register is write-only).
/// - `Action`: the standard's own §2.8.10 example, an enumeration that polls
///   while its value is a self-clearing entry.
const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Float Name="DeviceTemperature">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>DeviceTemperature_FloatVal</pValue>
    <Unit>C</Unit>
  </Float>
  <Converter Name="DeviceTemperature_FloatVal">
    <FormulaTo>FROM*256</FormulaTo>
    <FormulaFrom>TO/256</FormulaFrom>
    <pValue>DeviceTemperature_Val</pValue>
    <Slope>Increasing</Slope>
  </Converter>
  <IntReg Name="DeviceTemperature_Val">
    <Address>0x1000</Address><Length>4</Length><AccessMode>RO</AccessMode>
    <Cachable>NoCache</Cachable><PollingTime>500</PollingTime>
    <Sign>Signed</Sign><Endianess>BigEndian</Endianess>
  </IntReg>

  <Float Name="ExposureTime">
    <pValue>ExposureTime_FloatVal</pValue>
    <Unit>us</Unit>
  </Float>
  <Converter Name="ExposureTime_FloatVal">
    <FormulaTo>FROM</FormulaTo>
    <FormulaFrom>TO</FormulaFrom>
    <pValue>ExposureTime_Val</pValue>
    <Slope>Increasing</Slope>
  </Converter>
  <IntReg Name="ExposureTime_Val">
    <Address>0x1004</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Cachable>WriteAround</Cachable><PollingTime>500</PollingTime>
    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>

  <Integer Name="Width">
    <pValue>WidthReg</pValue><Min>1</Min><Max>4096</Max>
  </Integer>
  <IntReg Name="WidthReg">
    <Address>0x1008</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>

  <Float Name="Gain">
    <pValue>GainReg</pValue><Min>0</Min><Max>48</Max>
  </Float>
  <FloatReg Name="GainReg">
    <Address>0x1010</Address><Length>8</Length><AccessMode>RW</AccessMode>
    <Endianess>BigEndian</Endianess>
  </FloatReg>

  <MaskedIntReg Name="LineOut">
    <Address>0x1020</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Cachable>NoCache</Cachable>
    <Bit>0</Bit><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess>
  </MaskedIntReg>

  <Command Name="UserSetLoad">
    <pValue>UserSetLoadReg</pValue>
    <CommandValue>1</CommandValue>
    <PollingTime>5</PollingTime>
  </Command>
  <IntReg Name="UserSetLoadReg">
    <Address>0x1030</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>

  <Enumeration Name="Action">
    <EnumEntry Name="Idle"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Active"><Value>1</Value><IsSelfClearing>Yes</IsSelfClearing></EnumEntry>
    <pValue>ActionReg</pValue>
    <PollingTime>10</PollingTime>
  </Enumeration>
  <IntReg Name="ActionReg">
    <Address>0x1040</Address><Length>4</Length><AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>
</RegisterDescription>
"#;

const TEMPERATURE: u64 = 0x1000;
const EXPOSURE: u64 = 0x1004;
const WIDTH: u64 = 0x1008;
const GAIN: u64 = 0x1010;
const LINES: u64 = 0x1020;
const USER_SET_LOAD: u64 = 0x1030;
const ACTION: u64 = 0x1040;

/// The device: memory the test changes on the camera's behalf, read counts
/// per address, and the rounding a camera applies to some writes.
#[derive(Default)]
struct Device {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
    reads: RefCell<HashMap<u64, usize>>,
}

impl Device {
    fn new() -> Self {
        let device = Self::default();
        device.set_u32(TEMPERATURE, 40 * 256);
        device.set_u32(EXPOSURE, 5000);
        device.set_u32(WIDTH, 640);
        device.set_f64(GAIN, 0.0);
        device.set_u32_le(LINES, 0);
        device.set_u32(USER_SET_LOAD, 0);
        device.set_u32(ACTION, 0);
        device
    }

    /// The camera changes a register on its own.
    fn set_u32(&self, addr: u64, value: u32) {
        self.regs
            .borrow_mut()
            .insert(addr, value.to_be_bytes().to_vec());
    }

    fn set_u32_le(&self, addr: u64, value: u32) {
        self.regs
            .borrow_mut()
            .insert(addr, value.to_le_bytes().to_vec());
    }

    fn set_f64(&self, addr: u64, value: f64) {
        self.regs
            .borrow_mut()
            .insert(addr, value.to_be_bytes().to_vec());
    }

    /// What the device holds now.
    fn u32(&self, addr: u64) -> u32 {
        u32::from_be_bytes(self.regs.borrow()[&addr].clone().try_into().unwrap())
    }

    fn u32_le(&self, addr: u64) -> u32 {
        u32::from_le_bytes(self.regs.borrow()[&addr].clone().try_into().unwrap())
    }

    fn f64(&self, addr: u64) -> f64 {
        f64::from_be_bytes(self.regs.borrow()[&addr].clone().try_into().unwrap())
    }

    fn reads(&self, addr: u64) -> usize {
        self.reads.borrow().get(&addr).copied().unwrap_or(0)
    }
}

impl RegisterIo for Device {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        *self.reads.borrow_mut().entry(addr).or_default() += 1;
        let mut bytes = self.regs.borrow().get(&addr).cloned().unwrap_or_default();
        bytes.resize(len, 0);
        Ok(bytes)
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let stored = match addr {
            // The camera rounds an exposure to a multiple of 10 us, and a gain
            // to a whole dB: what it stores is not what it was sent.
            EXPOSURE => {
                let value = u32::from_be_bytes(data.try_into().unwrap());
                (value / 10 * 10).to_be_bytes().to_vec()
            }
            GAIN => {
                let value = f64::from_be_bytes(data.try_into().unwrap());
                value.floor().to_be_bytes().to_vec()
            }
            // A command the device has accepted reads back its value until
            // the device finishes; the test clears it.
            _ => data.to_vec(),
        };
        self.regs.borrow_mut().insert(addr, stored);
        Ok(())
    }
}

fn map(clock: &Arc<ManualClock>) -> NodeMap {
    let mut map =
        NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("build");
    map.set_clock(clock.clone());
    map
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `NoCache`: every read asks the device, through the converter and the
/// float in front of the register too — on `main` the converter answered
/// from its own cache and the register from its.
#[test]
fn a_no_cache_value_is_read_from_the_device_every_time() {
    let clock = Arc::new(ManualClock::new());
    let map = map(&clock);
    let device = Device::new();
    assert_eq!(map.get_float("DeviceTemperature", &device).unwrap(), 40.0);
    device.set_u32(TEMPERATURE, 41 * 256 + 128);
    assert_eq!(
        map.get_float("DeviceTemperature", &device).unwrap(),
        f64::from(device.u32(TEMPERATURE)) / 256.0,
        "the second read must show what the device holds now"
    );
    assert_eq!(device.reads(TEMPERATURE), 2);
}

/// `WriteAround`: a write leaves nothing cached, so the next read shows what
/// the camera made of it; a read is cached as usual.
#[test]
fn a_write_around_register_shows_what_the_camera_stored() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    map.set_float("ExposureTime", 1234.0, &device).unwrap();
    assert_eq!(device.u32(EXPOSURE), 1230, "the camera rounded it");
    assert_eq!(
        map.get_float("ExposureTime", &device).unwrap(),
        f64::from(device.u32(EXPOSURE))
    );
    let reads = device.reads(EXPOSURE);
    assert_eq!(map.get_float("ExposureTime", &device).unwrap(), 1230.0);
    assert_eq!(device.reads(EXPOSURE), reads, "a read value is cached");
}

/// A `<FloatReg>` that declares no `<Cachable>` is `WriteAround` (GenICam
/// Standard v2.1.1 §3.2); an `<IntReg>` is `WriteThrough`, so a write to one
/// is answered from the cache — the control.
#[test]
fn a_float_reg_defaults_to_write_around_and_an_int_reg_to_write_through() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    map.set_float("Gain", 12.5, &device).unwrap();
    assert_eq!(device.f64(GAIN), 12.0, "the camera rounded it");
    assert_eq!(map.get_float("Gain", &device).unwrap(), device.f64(GAIN));

    map.set_integer("Width", 320, &device).unwrap();
    assert_eq!(map.get_integer("Width", &device).unwrap(), 320);
    assert_eq!(
        device.reads(WIDTH),
        0,
        "write-through: the write is the cache"
    );
}

/// A read-modify-write of a bit in a `NoCache` register reads the register
/// first, so the bits the device drives survive — a value read earlier is
/// stale by definition. Checked on the device's own register.
#[test]
fn a_masked_write_to_a_no_cache_register_keeps_the_bits_the_device_set_since() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    assert_eq!(map.get_integer("LineOut", &device).unwrap(), 0);
    device.set_u32_le(LINES, 0b10); // an input line goes high
    map.set_integer("LineOut", 1, &device).unwrap();
    assert_eq!(device.u32_le(LINES), 0b11, "the input bit must survive");
}

/// `<PollingTime>`: a cached value is the device's for that long, then read
/// again — through the converter in front of the register too.
#[test]
fn a_polled_value_expires_after_its_polling_time() {
    let clock = Arc::new(ManualClock::new());
    let map = map(&clock);
    let device = Device::new();
    assert_eq!(map.get_float("ExposureTime", &device).unwrap(), 5000.0);
    device.set_u32(EXPOSURE, 7000); // auto exposure moved it
    clock.advance(ms(499));
    assert_eq!(
        map.get_float("ExposureTime", &device).unwrap(),
        5000.0,
        "still within its polling time"
    );
    assert_eq!(device.reads(EXPOSURE), 1);
    clock.advance(ms(1));
    assert_eq!(
        map.get_float("ExposureTime", &device).unwrap(),
        f64::from(device.u32(EXPOSURE)),
        "polling time over: the device's value"
    );
    assert_eq!(device.reads(EXPOSURE), 2);
}

/// What a viewer needs to know to poll: the polling time and caching mode of
/// a feature are those of the registers its value is read through.
#[test]
fn polling_time_and_caching_mode_follow_the_value_path() {
    let clock = Arc::new(ManualClock::new());
    let map = map(&clock);
    use CachingMode::{NoCache, WriteAround, WriteThrough};
    for (name, mode, polling) in [
        ("DeviceTemperature", NoCache, Some(ms(500))),
        ("DeviceTemperature_FloatVal", NoCache, Some(ms(500))),
        ("DeviceTemperature_Val", NoCache, Some(ms(500))),
        ("ExposureTime", WriteAround, Some(ms(500))),
        ("Width", WriteThrough, None),
        ("Gain", WriteAround, None),
        ("UserSetLoad", WriteThrough, Some(ms(5))),
        ("Action", WriteThrough, Some(ms(10))),
    ] {
        assert_eq!(map.caching_mode(name), Some(mode), "{name}");
        assert_eq!(map.polling_time(name), polling, "{name}");
    }
    assert_eq!(map.caching_mode("NoSuchNode"), None);
    assert_eq!(map.polling_time("NoSuchNode"), None);
}

/// GA-40: a generation counter instead of callbacks. A write and an
/// invalidation are changes, to the node and to what depends on it; a read is
/// not.
#[test]
fn changed_since_reports_what_writes_and_invalidations_touched() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    let start = map.generation();
    map.get_integer("Width", &device).unwrap();
    let (changed, now) = map.changed_since(start);
    assert!(changed.is_empty(), "a read changes nothing: {changed:?}");
    assert_eq!(now, start);

    map.set_integer("Width", 320, &device).unwrap();
    let (changed, after_write) = map.changed_since(now);
    assert_eq!(changed, ["Width", "WidthReg"], "document order");
    assert!(after_write > now);
    assert!(map.changed_since(after_write).0.is_empty());

    map.invalidate("DeviceTemperature_Val");
    let (changed, after_invalidate) = map.changed_since(after_write);
    assert_eq!(
        changed,
        [
            "DeviceTemperature",
            "DeviceTemperature_FloatVal",
            "DeviceTemperature_Val"
        ]
    );

    map.invalidate_all();
    let (changed, _) = map.changed_since(after_invalidate);
    assert_eq!(changed.len(), map.node_names().count(), "everything");
    // An old generation still sees everything since.
    assert!(map.changed_since(start).0.contains(&"Width".to_string()));
}

/// `poll`, in the spirit of GenApi's `INodeMap::Poll`: the polled values
/// whose polling time has passed since they were read are invalidated, and
/// their names — with everything read through them — returned, once.
#[test]
fn poll_reports_each_polled_value_once_its_time_has_passed() {
    let clock = Arc::new(ManualClock::new());
    let map = map(&clock);
    let device = Device::new();
    assert!(map.poll().is_empty(), "nothing read, nothing to refresh");
    map.get_float("DeviceTemperature", &device).unwrap();
    let generation = map.generation();
    clock.advance(ms(499));
    assert!(map.poll().is_empty());
    clock.advance(ms(1));
    assert_eq!(
        map.poll(),
        [
            "DeviceTemperature",
            "DeviceTemperature_FloatVal",
            "DeviceTemperature_Val"
        ]
    );
    assert_eq!(
        map.changed_since(generation).0,
        [
            "DeviceTemperature",
            "DeviceTemperature_FloatVal",
            "DeviceTemperature_Val"
        ],
        "a poll is a change"
    );
    assert!(map.poll().is_empty(), "reported once until read again");
    map.get_float("DeviceTemperature", &device).unwrap();
    clock.advance(ms(500));
    assert_eq!(map.poll().len(), 3);
}

/// A command's `<PollingTime>` polls it while it runs (§2.8.8): from the
/// execute until `is_done` finds it finished.
#[test]
fn a_running_command_is_polled_until_it_is_done() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    map.exec_command("UserSetLoad", &device).unwrap();
    clock.advance(ms(5));
    assert!(map.poll().contains(&"UserSetLoad".to_string()), "running");
    assert!(!map.is_done("UserSetLoad", &device).unwrap());
    clock.advance(ms(5));
    assert!(
        map.poll().contains(&"UserSetLoad".to_string()),
        "still running"
    );
    device.set_u32(USER_SET_LOAD, 0); // the device finished
    assert!(map.is_done("UserSetLoad", &device).unwrap());
    clock.advance(ms(5));
    assert!(map.poll().is_empty(), "done: polling stops");
}

/// §2.8.10's example: while `Action` is `Active`, a self-clearing entry, it
/// is read from the device every time and polled; once it reads `Idle`, the
/// cache is active again.
#[test]
fn an_enumeration_polls_only_while_its_entry_is_self_clearing() {
    let clock = Arc::new(ManualClock::new());
    let mut map = map(&clock);
    let device = Device::new();
    map.set_enum("Action", "Active", &device).unwrap();
    assert_eq!(map.get_enum("Action", &device).unwrap(), "Active");
    let reads = device.reads(ACTION);
    assert_eq!(map.get_enum("Action", &device).unwrap(), "Active");
    assert_eq!(device.reads(ACTION), reads + 1, "Active: past the cache");
    clock.advance(ms(10));
    assert!(map.poll().contains(&"Action".to_string()));

    device.set_u32(ACTION, 0); // the action finished
    assert_eq!(map.get_enum("Action", &device).unwrap(), "Idle");
    let reads = device.reads(ACTION);
    assert_eq!(map.get_enum("Action", &device).unwrap(), "Idle");
    assert_eq!(device.reads(ACTION), reads, "Idle: cached again");
    clock.advance(ms(10));
    assert!(map.poll().is_empty(), "Idle: not polled");
}

/// The feature state a viewer reads carries what it needs to poll.
#[test]
fn the_feature_state_says_how_to_refresh_it() {
    let clock = Arc::new(ManualClock::new());
    let map = map(&clock);
    let device = Device::new();
    let state = map.feature_state("DeviceTemperature", &device).unwrap();
    assert_eq!(state.polling_time, Some(ms(500)));
    assert_eq!(state.caching, CachingMode::NoCache);
    let state = map.feature_state("Width", &device).unwrap();
    assert_eq!(state.polling_time, None);
    assert_eq!(state.caching, CachingMode::WriteThrough);
}
