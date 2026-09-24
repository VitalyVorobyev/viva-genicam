//! In-memory bootstrap register map and embedded GenApi XML.
//!
//! # Register Address Map
//!
//! | Address     | Length | Feature                         | Type     |
//! |-------------|--------|---------------------------------|----------|
//! | `0x0000`    | 4      | Version (RO)                    | u32 BE   |
//! | `0x0004`    | 4      | DeviceMode (RO)                 | u32 BE   |
//! | `0x0008`    | 4      | DeviceMACAddressHigh (RO)       | u32 BE   |
//! | `0x000c`    | 4      | DeviceMACAddressLow (RO)        | u32 BE   |
//! | `0x0010`    | 4      | SupportedIPConfiguration (RO)   | u32 BE   |
//! | `0x0014`    | 4      | CurrentIPConfiguration          | u32 BE   |
//! | `0x0024`    | 4      | CurrentIPAddress (RO)           | IPv4     |
//! | `0x0034`    | 4      | CurrentSubnetMask (RO)          | IPv4     |
//! | `0x0044`    | 4      | CurrentDefaultGateway (RO)      | IPv4     |
//! | `0x0900`    | 4      | GevNumberOfMessageChannels (RO) | u32 BE   |
//! | `0x0904`    | 4      | GevNumberOfStreamChannels (RO)  | u32 BE   |
//! | `0x0a00`    | 4      | CCP (Control Channel Privilege) | u32 BE   |
//! | `0x0938`    | 4      | Heartbeat Timeout               | u32 BE   |
//! | `0x0b00`    | 4      | GevMCP (message channel port)   | u32 BE   |
//! | `0x0b10`    | 4      | GevMCDA (message channel addr)  | u32 BE   |
//! | `0x0d00+`   | varies | Stream Channel 0 registers      | u32 BE   |
//! | `0x20000`   | 4      | Width                           | u32 BE   |
//! | `0x20004`   | 4      | Height                          | u32 BE   |
//! | `0x20008`   | 4      | PixelFormat                     | u32 BE   |
//! | `0x2000c`   | 4      | OffsetX                         | u32 BE   |
//! | `0x20010`   | 4      | OffsetY                         | u32 BE   |
//! | `0x20014`   | 4      | SensorWidth (RO)                | u32 BE   |
//! | `0x20018`   | 4      | SensorHeight (RO)               | u32 BE   |
//! | `0x20020`   | 4      | AcquisitionMode                 | u32 BE   |
//! | `0x20024`   | 4      | AcquisitionStart (command)      | u32 BE   |
//! | `0x20028`   | 4      | AcquisitionStop (command)       | u32 BE   |
//! | `0x2002c`   | 4      | AcquisitionFrameRate            | f32→u32  |
//! | `0x20030`   | 8      | ExposureTime                    | f64 BE   |
//! | `0x20038`   | 4      | ExposureAuto                    | u32 BE   |
//! | `0x20040`   | 8      | Gain                            | f64 BE   |
//! | `0x20048`   | 4      | GainAuto                        | u32 BE   |
//! | `0x20050`   | 4      | BlackLevel                      | u32 BE   |
//! | `0x20054`   | 4      | AcquisitionFrameRateEnable      | u32 BE   |
//! | `0x20058`   | 4      | SensorType                      | u32 BE   |
//! | `0x20060`   | 4      | GevTimestampTickFrequency (RO)  | u32 BE   |
//! | `0x20068`   | 8      | GevTimestampValue (RO)          | u64 BE   |
//! | `0x20070`   | 4      | TimestampLatch (command)        | u32 BE   |
//! | `0x20078`   | 8      | GevIEEE1588OffsetFromMaster…    | u64 BE   |
//! | `0x20080`   | 4      | ChunkModeActive                 | u32 BE   |
//! | `0x20084`   | 4      | ChunkSelector                   | u32 BE   |
//! | `0x20088`   | 4      | ChunkEnable                     | u32 BE   |
//! | `0x200a0`   | 4      | EventSelector                   | u32 BE   |
//! | `0x200a4`   | 4      | EventNotification (per selector)| u32 BE   |
//! | `0x200a8`   | 4      | UserSetSelector                 | u32 BE   |
//! | `0x200ac`   | 4      | UserSetLoad (command, pValue)   | u32 BE   |
//! | `0x200b0`   | 4      | SoftwareSignal*Pulse (WO bits)  | u32 BE   |
//! | `0x20100`   | 4      | WidthMin (RO)                   | u32 BE   |
//! | `0x20104`   | 4      | WidthMax (RO)                   | u32 BE   |
//! | `0x20108`   | 4      | HeightMin (RO)                  | u32 BE   |
//! | `0x2010c`   | 4      | HeightMax (RO)                  | u32 BE   |
//! | `0x20110`   | 8      | ChunkTimestamp_Val (RO)         | u64 **LE** |
//! | `0x20118`   | 4      | ChunkWidth_Val (RO)             | u32 **LE** |
//! | `0x20200`   | 32     | DeviceModelName (RO)            | string   |
//! | `0x20220`   | 32     | DeviceVendorName (RO)           | string   |
//! | `0x20240`   | 16     | DeviceSerialNumber (RO)         | string   |
//! | `0x20260`   | 32     | DeviceFirmwareVersion (RO)      | string   |
//! | `0x20280`   | 32     | DeviceID (RO)                   | string   |

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use crate::gvcp_server::FAKE_MAC;

/// Bootstrap register addresses (GigE Vision specification).
pub const VERSION: u64 = 0x0000;
pub const DEVICE_MODE: u64 = 0x0004;
/// Top two MAC bytes, right-aligned in the 32-bit register.
pub const DEVICE_MAC_HIGH: u64 = 0x0008;
/// Bottom four MAC bytes.
pub const DEVICE_MAC_LOW: u64 = 0x000C;
pub const SUPPORTED_IP_CONFIG: u64 = 0x0010;
pub const CURRENT_IP_CONFIG: u64 = 0x0014;
pub const CURRENT_IP_ADDRESS: u64 = 0x0024;
pub const CURRENT_SUBNET_MASK: u64 = 0x0034;
pub const CURRENT_DEFAULT_GATEWAY: u64 = 0x0044;
/// `GevNumberOfMessageChannels` — the fake implements one.
pub const NUMBER_OF_MESSAGE_CHANNELS: u64 = 0x0900;
/// `GevNumberOfStreamChannels` — the fake implements one.
pub const NUMBER_OF_STREAM_CHANNELS: u64 = 0x0904;

/// `DeviceMode`: big-endian (bit 31), character set 1 (UTF-8), class
/// Transmitter (0).
pub const DEVICE_MODE_VALUE: u32 = 0x8000_0001;
pub const PERSISTENT_IP_ADDRESS: u64 = 0x064C;
pub const PERSISTENT_SUBNET_MASK: u64 = 0x065C;
pub const PERSISTENT_DEFAULT_GATEWAY: u64 = 0x066C;
pub const CCP: u64 = 0x0a00;
pub const HEARTBEAT_TIMEOUT: u64 = 0x0938;
/// Message channel destination port (`GevMCP`), port in the low 16 bits.
pub const MESSAGE_CHANNEL_PORT: u64 = 0x0b00;
/// Message channel destination address (`GevMCDA`).
pub const MESSAGE_CHANNEL_ADDRESS: u64 = 0x0b10;
pub const STREAM_CHANNEL_BASE: u64 = 0x0d00;
/// Address stride between GigE Vision stream channel register blocks.
pub const STREAM_CHANNEL_STRIDE: u64 = 0x40;
pub const SCP_HOST_PORT: u64 = 0x00;
pub const SCP_PACKET_SIZE: u64 = 0x04;
pub const SCP_PACKET_DELAY: u64 = 0x08;
pub const SCP_DEST_ADDR: u64 = 0x18;

/// First XML URL register address and length.
pub const FIRST_URL_REG: u64 = 0x0200;
pub const URL_REG_LEN: usize = 512;

/// Address where the actual XML blob is stored in the register space.
pub const XML_BLOB_BASE: u64 = 0x1_0000;

// ── Feature register addresses ──────────────────────────────────────────────

/// Image format registers.
pub const REG_WIDTH: u64 = 0x20000;
pub const REG_HEIGHT: u64 = 0x20004;
pub const REG_PIXEL_FORMAT: u64 = 0x20008;
pub const REG_OFFSET_X: u64 = 0x2000c;
pub const REG_OFFSET_Y: u64 = 0x20010;
pub const REG_SENSOR_WIDTH: u64 = 0x20014;
pub const REG_SENSOR_HEIGHT: u64 = 0x20018;

/// Acquisition registers.
pub const REG_ACQ_MODE: u64 = 0x20020;
pub const REG_ACQ_START: u64 = 0x20024;
pub const REG_ACQ_STOP: u64 = 0x20028;
pub const REG_ACQ_FRAME_RATE: u64 = 0x2002c;

/// Analog control registers.
pub const REG_EXPOSURE_TIME: u64 = 0x20030;
pub const REG_EXPOSURE_AUTO: u64 = 0x20038;
pub const REG_GAIN: u64 = 0x20040;
pub const REG_GAIN_AUTO: u64 = 0x20048;
pub const REG_BLACK_LEVEL: u64 = 0x20050;

/// Predicate-gating registers driving realistic feature behaviour.
///
/// `REG_ACQ_FRAME_RATE_ENABLE` backs the SFNC `AcquisitionFrameRateEnable`
/// Boolean and gates `AcquisitionFrameRate` via `pIsAvailable`.
/// `REG_SENSOR_TYPE` backs a `SensorType` enumeration (Monochrome / BayerRG /
/// Color) that gates `PixelFormat` entries via `pIsImplemented`. On real
/// hardware `SensorType` would be read-only sensor metadata, but exposing it
/// as RW here keeps the fake camera a configurable simulator.
pub const REG_ACQ_FRAME_RATE_ENABLE: u64 = 0x20054;
pub const REG_SENSOR_TYPE: u64 = 0x20058;

/// Device capability inquiry bits, exposed through a `<StructReg>` whose
/// address is `<pAddress>` (a base node) plus a fixed `<Address>` offset — the
/// shape Point Grey, FLIR and Hikrobot use for their inquiry blocks.
pub const REG_DEVICE_CAPS: u64 = 0x2005c;

/// Which stream channel the `GevSCP*` features address.
///
/// Backs a `<pIndex>` term, so changing it moves those registers by
/// [`STREAM_CHANNEL_STRIDE`].
pub const REG_STREAM_CHANNEL_SELECTOR: u64 = 0x20090;

/// Timestamp registers.
pub const REG_TIMESTAMP_FREQ: u64 = 0x20060;
pub const REG_TIMESTAMP_VALUE: u64 = 0x20068;
pub const REG_TIMESTAMP_LATCH: u64 = 0x20070;

/// A PTP offset-from-master, copied in shape from the FLIR BFS-PGE-31S4C-C
/// description in the corpus (`<Length>8</Length>`, `<Sign>Unsigned</Sign>`,
/// `<Endianess>BigEndian</Endianess>`). It is the node issue #140 was filed
/// against, and it is declared unsigned although a clock offset is signed by
/// nature — so its top bit is set whenever the slave leads the master. That
/// combination is what made the node unreadable.
pub const REG_PTP_OFFSET_LATCHED: u64 = 0x20078;

/// Chunk value registers, declared **little-endian** exactly as FLIR, Point
/// Grey and Hikrobot declare theirs. 311 plain `<IntReg>` declarations across
/// 16 of the 38 corpus documents are of this shape; before GA-28 every one of
/// them decoded byte-swapped, and nothing in the tree could notice.
pub const REG_CHUNK_TIMESTAMP_LE: u64 = 0x20110;
pub const REG_CHUNK_WIDTH_LE: u64 = 0x20118;

/// Value latched into [`REG_PTP_OFFSET_LATCHED`]: the slave leading the master
/// by 1 234 567 ns. Stored as the two's-complement bit pattern a camera would
/// put on the wire, so the test asserts the bytes and the decoded value
/// separately rather than round-tripping through our own codec.
pub const PTP_OFFSET_NS: i64 = -1_234_567;

/// Value latched into [`REG_CHUNK_TIMESTAMP_LE`]. Chosen so that reading it in
/// the wrong byte order yields a plausible positive number
/// (`0x7856_3412_0000_0000`) rather than an error — a byte-order defect that
/// announces itself is not the one that shipped.
pub const CHUNK_TIMESTAMP_TICKS: u64 = 0x0000_0000_1234_5678;

/// Value latched into [`REG_CHUNK_WIDTH_LE`]. Most of the 311 are 4 bytes
/// wide, so the common width gets its own node.
pub const CHUNK_WIDTH_PX: u32 = 1440;

/// Chunk data registers.
pub const REG_CHUNK_MODE_ACTIVE: u64 = 0x20080;
pub const REG_CHUNK_SELECTOR: u64 = 0x20084;
pub const REG_CHUNK_ENABLE: u64 = 0x20088;

/// `EventSelector` backing register: a GigE Vision event identifier.
/// `REG_FEATURE_STATUS` is a FLIR-shaped feature-status word: one big-endian
/// register whose individual bits say whether a feature is implemented,
/// available and locked, addressed by `<MaskedIntReg>` + `<Bit>`.
///
/// It exists so the fake can disagree with us about bit numbering. Every other
/// predicate here is an `<IntSwissKnife>` or a `<StructEntry>`, and both of
/// those took the code path that was already correct — so the whole suite
/// passed while `<MaskedIntReg>` read big-endian registers off the wrong end on
/// real hardware (issue #120). GenICam counts `<Bit>` from the MSB on a
/// big-endian register, so bit 0 is `0x8000_0000`.
pub const REG_FEATURE_STATUS: u64 = 0x2009c;

pub const REG_EVENT_SELECTOR: u64 = 0x200a0;
/// `EventNotification` backing register for the selected event (0 = Off, 1 = On).
///
/// A *selected* feature: reads and writes apply to whichever event
/// [`REG_EVENT_SELECTOR`] currently names, so the register is backed by a set
/// of enabled event ids rather than by one stored word. Query it with
/// [`RegisterMap::event_notification_on`].
pub const REG_EVENT_NOTIFICATION: u64 = 0x200a4;

/// `REG_USER_SET_SELECTOR` and `REG_USER_SET_LOAD` back the SFNC user-set
/// features, so the fake can answer the workflow issue #121 was filed about.
///
/// `UserSetLoad` is also the fake's first command that reaches its register
/// through `<pValue>` rather than a bare `<Address>`. That matters for backlog
/// `GA-10`: all 432 `<Command>` nodes in the vendor corpus use `<pValue>`, and
/// until now all three of the fake's used the direct-address path — so our
/// integration tests exercised only the path no real camera takes.
/// Exposure the fake boots with, and returns to on `UserSetLoad`.
pub const DEFAULT_EXPOSURE_US: f64 = 5000.0;

pub const REG_USER_SET_SELECTOR: u64 = 0x200a8;
/// See [`REG_USER_SET_SELECTOR`].
pub const REG_USER_SET_LOAD: u64 = 0x200ac;

/// A write-only register addressed bit by bit: a `<StructReg>` with
/// `AccessMode WO` whose `<StructEntry>` bits each pulse one software signal.
///
/// It is here so the fake can refuse a masked write that reads first
/// (backlog GA-31, [#135](https://github.com/VitalyVorobyev/viva-genicam/issues/135)).
/// Every other WO node in this XML is a whole-register command, and a
/// whole-register write never needs the other bits — so the read-modify-write
/// path had no in-tree coverage at all. A `<StructEntry>` inherits the
/// register's access mode and always has a bitfield, which is the shape the
/// vendor corpus uses.
pub const REG_SOFTWARE_SIGNAL_PULSE: u64 = 0x200b0;

/// Registers the XML declares `WO`. Reading any of them is refused with
/// `ACCESS_DENIED`, as a real device refuses it.
///
/// Listed explicitly rather than derived from the XML: the fake must be able to
/// disagree with our parser, and deriving the list through it would make the
/// two agree by construction (ADR-0019).
pub const WRITE_ONLY_REGISTERS: &[u64] = &[
    REG_ACQ_START,
    REG_ACQ_STOP,
    REG_TIMESTAMP_LATCH,
    REG_USER_SET_LOAD,
    REG_SOFTWARE_SIGNAL_PULSE,
];

/// Limit registers.
pub const REG_WIDTH_MIN: u64 = 0x20100;
pub const REG_WIDTH_MAX: u64 = 0x20104;
pub const REG_HEIGHT_MIN: u64 = 0x20108;
pub const REG_HEIGHT_MAX: u64 = 0x2010c;

/// Device info string registers.
pub const REG_DEVICE_MODEL_NAME: u64 = 0x20200;
pub const REG_DEVICE_VENDOR_NAME: u64 = 0x20220;
pub const REG_DEVICE_SERIAL_NUMBER: u64 = 0x20240;
pub const REG_DEVICE_FIRMWARE_VERSION: u64 = 0x20260;
pub const REG_DEVICE_ID: u64 = 0x20280;

// ── GenApi XML ──────────────────────────────────────────────────────────────

/// GenApi XML describing a realistic fake camera following SFNC conventions.
///
/// The XML is organized with proper SFNC category hierarchy:
///
/// ```text
/// Root
/// ├── DeviceControl        — model name, vendor, serial, firmware, device ID
/// ├── ImageFormatControl   — width, height, offset, pixel format, sensor size
/// ├── AcquisitionControl   — start/stop, mode, frame rate, exposure, auto
/// ├── AnalogControl        — gain, gain auto, black level
/// ├── TransportLayerControl — timestamp tick frequency, value, latch
/// └── ChunkDataControl     — chunk mode, selector, enable
/// ```
///
/// All feature registers use big-endian byte order. Register addresses are
/// documented in the module-level doc comment.
pub const FAKE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription
  ModelName="VivaCam Fake"
  VendorName="vitavision.dev"
  ToolTip="Simulated GigE Vision camera for testing"
  StandardNameSpace="GEV"
  SchemaMajorVersion="1"
  SchemaMinorVersion="1"
  SchemaSubMinorVersion="0"
  MajorVersion="1"
  MinorVersion="0"
  SubMinorVersion="0"
  ProductGuid="76697661-6361-6d00-0000-000000000000"
  VersionGuid="76697661-6361-6d00-0000-000000000001">

  <!-- ════════════════════════════════════════════════════════════════════
       Category Hierarchy (SFNC Standard)
       ════════════════════════════════════════════════════════════════════ -->

  <Category Name="Root" NameSpace="Standard">
    <pFeature>DeviceControl</pFeature>
    <pFeature>ImageFormatControl</pFeature>
    <pFeature>AcquisitionControl</pFeature>
    <pFeature>AnalogControl</pFeature>
    <pFeature>TransportLayerControl</pFeature>
    <pFeature>ChunkDataControl</pFeature>
    <pFeature>UserSetControl</pFeature>
  </Category>

  <Category Name="UserSetControl">
    <DisplayName>User Set Control</DisplayName>
    <pFeature>UserSetSelector</pFeature>
    <pFeature>UserSetLoad</pFeature>
  </Category>

  <Category Name="DeviceControl">
    <DisplayName>Device Control</DisplayName>
    <pFeature>DeviceVendorName</pFeature>
    <pFeature>DeviceModelName</pFeature>
    <pFeature>DeviceSerialNumber</pFeature>
    <pFeature>DeviceFirmwareVersion</pFeature>
    <pFeature>DeviceID</pFeature>
  </Category>

  <Category Name="ImageFormatControl">
    <DisplayName>Image Format Control</DisplayName>
    <pFeature>SensorType</pFeature>
    <pFeature>SensorWidth</pFeature>
    <pFeature>SensorHeight</pFeature>
    <pFeature>Width</pFeature>
    <pFeature>Height</pFeature>
    <pFeature>OffsetX</pFeature>
    <pFeature>OffsetY</pFeature>
    <pFeature>PixelFormat</pFeature>
  </Category>

  <Category Name="AcquisitionControl">
    <DisplayName>Acquisition Control</DisplayName>
    <pFeature>AcquisitionMode</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
    <pFeature>AcquisitionFrameRateEnable</pFeature>
    <pFeature>AcquisitionFrameRate</pFeature>
    <pFeature>ExposureTime</pFeature>
    <pFeature>ExposureAuto</pFeature>
  </Category>

  <Category Name="AnalogControl">
    <DisplayName>Analog Control</DisplayName>
    <pFeature>Gain</pFeature>
    <pFeature>GainAuto</pFeature>
    <pFeature>BlackLevel</pFeature>
  </Category>

  <Category Name="TransportLayerControl">
    <DisplayName>Transport Layer Control</DisplayName>
    <pFeature>GevTimestampTickFrequency</pFeature>
    <pFeature>GevTimestampValue</pFeature>
    <pFeature>TimestampLatch</pFeature>
    <pFeature>GevIEEE1588OffsetFromMasterLatched_Val</pFeature>
    <pFeature>ChunkTimestamp_Val</pFeature>
    <pFeature>ChunkWidth_Val</pFeature>
  </Category>

  <Category Name="ChunkDataControl">
    <DisplayName>Chunk Data Control</DisplayName>
    <pFeature>ChunkModeActive</pFeature>
    <pFeature>ChunkSelector</pFeature>
    <pFeature>ChunkEnable</pFeature>
  </Category>

  <Category Name="EventControl">
    <DisplayName>Event Control</DisplayName>
    <pFeature>EventSelector</pFeature>
    <pFeature>EventNotification</pFeature>
  </Category>

  <!-- ════════════════════════════════════════════════════════════════════
       Device Control Features
       ════════════════════════════════════════════════════════════════════ -->

  <String Name="DeviceVendorName" NameSpace="Standard">
    <ToolTip>Name of the device vendor</ToolTip>
    <Address>0x20220</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
  </String>

  <String Name="DeviceModelName" NameSpace="Standard">
    <ToolTip>Name of the device model</ToolTip>
    <Address>0x20200</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
  </String>

  <String Name="DeviceSerialNumber" NameSpace="Standard">
    <ToolTip>Serial number of the device</ToolTip>
    <Address>0x20240</Address>
    <Length>16</Length>
    <AccessMode>RO</AccessMode>
  </String>

  <String Name="DeviceFirmwareVersion" NameSpace="Standard">
    <ToolTip>Firmware version of the device</ToolTip>
    <Address>0x20260</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
  </String>

  <String Name="DeviceID" NameSpace="Standard">
    <ToolTip>User-configurable device identifier</ToolTip>
    <Address>0x20280</Address>
    <Length>32</Length>
    <AccessMode>RO</AccessMode>
  </String>

  <!-- ════════════════════════════════════════════════════════════════════
       Image Format Control Features
       ════════════════════════════════════════════════════════════════════ -->

  <Integer Name="SensorWidth" NameSpace="Standard">
    <ToolTip>Physical sensor width in pixels</ToolTip>
    <Address>0x20014</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Min>1</Min>
    <Max>4096</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Integer Name="SensorHeight" NameSpace="Standard">
    <ToolTip>Physical sensor height in pixels</ToolTip>
    <Address>0x20018</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Min>1</Min>
    <Max>4096</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Integer Name="Width" NameSpace="Standard">
    <ToolTip>Width of the image in pixels</ToolTip>
    <Address>0x20000</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pMin>WidthMin</pMin>
    <pMax>WidthMax</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <IntReg Name="WidthMin"><Address>0x20100</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <IntReg Name="WidthMax"><Address>0x20104</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="Height" NameSpace="Standard">
    <ToolTip>Height of the image in pixels</ToolTip>
    <Address>0x20004</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pMin>HeightMin</pMin>
    <pMax>HeightMax</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <IntReg Name="HeightMin"><Address>0x20108</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <IntReg Name="HeightMax"><Address>0x2010c</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="OffsetX" NameSpace="Standard">
    <ToolTip>Horizontal offset from the sensor origin</ToolTip>
    <Address>0x2000c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <Max>4096</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Integer Name="OffsetY" NameSpace="Standard">
    <ToolTip>Vertical offset from the sensor origin</ToolTip>
    <Address>0x20010</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <Max>4096</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Enumeration Name="SensorType" NameSpace="Standard">
    <ToolTip>Sensor variant (Monochrome / BayerRG / Color). On real hardware this would be read-only sensor metadata; here it is RW so tests can reconfigure the simulator.</ToolTip>
    <EnumEntry Name="Monochrome"><Value>0</Value></EnumEntry>
    <EnumEntry Name="BayerRG"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Color"><Value>2</Value></EnumEntry>
    <pValue>SensorTypeReg</pValue>
  </Enumeration>
  <IntReg Name="SensorTypeReg"><Address>0x20058</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Enumeration Name="PixelFormat" NameSpace="Standard">
    <ToolTip>Format of the pixel data — entries are gated by SensorType</ToolTip>
    <EnumEntry Name="Mono8" NameSpace="Standard">
      <Value>0x01080001</Value>
      <pIsImplemented>PfMono8Avail</pIsImplemented>
    </EnumEntry>
    <EnumEntry Name="Mono16" NameSpace="Standard">
      <Value>0x01100007</Value>
      <pIsImplemented>PfMono16Avail</pIsImplemented>
    </EnumEntry>
    <EnumEntry Name="RGB8" NameSpace="Standard">
      <Value>0x02180014</Value>
      <pIsImplemented>PfRGB8Avail</pIsImplemented>
    </EnumEntry>
    <EnumEntry Name="BayerRG8" NameSpace="Standard">
      <Value>0x01080009</Value>
      <pIsImplemented>PfBayerRG8Avail</pIsImplemented>
    </EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>
  <IntReg Name="PixelFormatReg"><Address>0x20008</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- PixelFormat entry availability driven by SensorType:
         Monochrome (0) → Mono8, Mono16
         BayerRG    (1) → BayerRG8
         Color      (2) → RGB8 -->
  <IntSwissKnife Name="PfMono8Avail">
    <Formula>ST = 0</Formula>
    <pVariable Name="ST">SensorTypeReg</pVariable>
  </IntSwissKnife>
  <IntSwissKnife Name="PfMono16Avail">
    <Formula>ST = 0</Formula>
    <pVariable Name="ST">SensorTypeReg</pVariable>
  </IntSwissKnife>
  <IntSwissKnife Name="PfBayerRG8Avail">
    <Formula>ST = 1</Formula>
    <pVariable Name="ST">SensorTypeReg</pVariable>
  </IntSwissKnife>
  <IntSwissKnife Name="PfRGB8Avail">
    <Formula>ST = 2</Formula>
    <pVariable Name="ST">SensorTypeReg</pVariable>
  </IntSwissKnife>

  <!-- ════════════════════════════════════════════════════════════════════
       Device capability inquiry bits

       Real inquiry blocks are addressed as `<pAddress>` (the block base) plus
       a fixed `<Address>` offset, and the individual bits come from a
       <StructReg>. Both terms contribute: keeping only one reads the wrong
       register, which is what made issue #35's camera misreport every
       capability it had.
       ════════════════════════════════════════════════════════════════════ -->
  <IntSwissKnife Name="DeviceRegBaseAddress">
    <Formula>0x20000</Formula>
  </IntSwissKnife>
  <StructReg Comment="Device capability inquiry">
    <pAddress>DeviceRegBaseAddress</pAddress>
    <Address>0x5C</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Endianess>BigEndian</Endianess>
    <StructEntry Name="FrameRateControlInq_Bit"><Bit>0</Bit></StructEntry>
    <StructEntry Name="ChunkSupportInq_Bit"><Bit>1</Bit></StructEntry>
    <StructEntry Name="SequencerInq_Bit"><Bit>2</Bit></StructEntry>
  </StructReg>

  <!-- ════════════════════════════════════════════════════════════════════
       GigE Vision stream channel registers

       Stream channel N lives at 0x0D00 + N * 0x40, so the packet size
       register is a fixed <Address> plus a <pIndex> scaled by the channel
       stride. Ignoring the <pIndex> term always addresses channel 0.
       ════════════════════════════════════════════════════════════════════ -->
  <Integer Name="GevStreamChannelSelector" NameSpace="Standard">
    <ToolTip>Stream channel the GevSCP* features address</ToolTip>
    <Address>0x20090</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Min>0</Min>
    <Max>1</Max>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <Integer Name="GevSCPSPacketSize" NameSpace="Standard">
    <ToolTip>Stream channel packet size in bytes</ToolTip>
    <Address>0x0D04</Address>
    <pIndex Offset="0x40">GevStreamChannelSelector</pIndex>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Min>0</Min>
    <Max>65535</Max>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <!-- ════════════════════════════════════════════════════════════════════
       Acquisition Control Features
       ════════════════════════════════════════════════════════════════════ -->

  <Enumeration Name="AcquisitionMode" NameSpace="Standard">
    <ToolTip>Camera acquisition mode</ToolTip>
    <EnumEntry Name="Continuous"><Value>0</Value></EnumEntry>
    <EnumEntry Name="SingleFrame"><Value>1</Value></EnumEntry>
    <EnumEntry Name="MultiFrame"><Value>2</Value></EnumEntry>
    <pValue>AcquisitionModeReg</pValue>
  </Enumeration>
  <IntReg Name="AcquisitionModeReg"><Address>0x20020</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Command Name="AcquisitionStart" NameSpace="Standard">
    <ToolTip>Start image acquisition</ToolTip>
    <Address>0x20024</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

  <Enumeration Name="UserSetSelector" NameSpace="Standard">
    <ToolTip>User set that UserSetLoad restores</ToolTip>
    <EnumEntry Name="Default"><Value>0</Value></EnumEntry>
    <EnumEntry Name="UserSet0"><Value>1</Value></EnumEntry>
    <pValue>UserSetSelectorReg</pValue>
  </Enumeration>
  <IntReg Name="UserSetSelectorReg"><Address>0x200a8</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- Reaches its register through <pValue>, unlike the three commands above.
       That is the shape every <Command> in the vendor corpus uses, and the one
       our tests had no example of (backlog GA-10). -->
  <Command Name="UserSetLoad" NameSpace="Standard">
    <ToolTip>Restore the selected user set</ToolTip>
    <pValue>UserSetLoadReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="UserSetLoadReg"><Address>0x200ac</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- A write-only register addressed bit by bit. Each entry inherits WO from
       the StructReg, so setting one bit cannot read the register for the
       others: the device refuses the read (backlog GA-31, issue #135). -->
  <StructReg Comment="Software signal pulse">
    <Address>0x200b0</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <Endianess>BigEndian</Endianess>
    <StructEntry Name="SoftwareSignal0Pulse"><Bit>0</Bit></StructEntry>
    <StructEntry Name="SoftwareSignal1Pulse"><Bit>1</Bit></StructEntry>
  </StructReg>

  <Command Name="AcquisitionStop" NameSpace="Standard">
    <ToolTip>Stop image acquisition</ToolTip>
    <Address>0x20028</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

  <Boolean Name="AcquisitionFrameRateEnable" NameSpace="Standard">
    <ToolTip>Enable manual control of AcquisitionFrameRate. When false, the frame rate is unavailable for read/write.</ToolTip>
    <pValue>AcquisitionFrameRateEnableReg</pValue>
  </Boolean>
  <IntReg Name="AcquisitionFrameRateEnableReg"><Address>0x20054</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
    <pIsImplemented>FrameRateControlInq_Bit</pIsImplemented>
  </IntReg>

  <Float Name="AcquisitionFrameRate" NameSpace="Standard">
    <ToolTip>Target frame rate in Hz</ToolTip>
    <Address>0x2002c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1.0</Min>
    <Max>120.0</Max>
    <Endianess>BigEndian</Endianess>
    <pIsAvailable>AcquisitionFrameRateEnable</pIsAvailable>
  </Float>

  <Float Name="ExposureTime" NameSpace="Standard">
    <ToolTip>Exposure time in microseconds — locked to RO when ExposureAuto is not Off</ToolTip>
    <Address>0x20030</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>10.0</Min>
    <Max>1000000.0</Max>
    <Endianess>BigEndian</Endianess>
    <pIsImplemented>ExposureTime_Imp</pIsImplemented>
    <pIsAvailable>ExposureTime_Avl</pIsAvailable>
    <pIsLocked>ExposureAutoActive</pIsLocked>
  </Float>

  <!-- Feature-status bits in one big-endian word, the shape FLIR ships and the
       shape that exposed issue #120. GenICam counts <Bit> from the MSB here, so
       bit 0 is 0x80000000 and bit 1 is 0x40000000; the fake boots this register
       to 0xC0000000. Read from the wrong end these both come out zero and
       ExposureTime becomes unavailable, which is what the reporter saw. -->
  <MaskedIntReg Name="ExposureTime_Imp">
    <Address>0x2009c</Address><Length>4</Length><AccessMode>RO</AccessMode>
    <Bit>0</Bit><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </MaskedIntReg>
  <MaskedIntReg Name="ExposureTime_Avl">
    <Address>0x2009c</Address><Length>4</Length><AccessMode>RO</AccessMode>
    <Bit>1</Bit><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </MaskedIntReg>

  <Enumeration Name="ExposureAuto" NameSpace="Standard">
    <ToolTip>Automatic exposure control</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Once"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Continuous"><Value>2</Value></EnumEntry>
    <pValue>ExposureAutoReg</pValue>
  </Enumeration>
  <IntReg Name="ExposureAutoReg"><Address>0x20038</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <IntSwissKnife Name="ExposureAutoActive">
    <Formula>EA &lt;&gt; 0</Formula>
    <pVariable Name="EA">ExposureAutoReg</pVariable>
  </IntSwissKnife>

  <!-- ════════════════════════════════════════════════════════════════════
       Analog Control Features
       ════════════════════════════════════════════════════════════════════ -->

  <Float Name="Gain" NameSpace="Standard">
    <!-- CDATA-wrapped tooltip, as shipped by several vendors: the literal `&`
         and `<` inside are legal here and must survive parsing (issue #45). -->
    <ToolTip><![CDATA[Gain applied to the image in dB — locked to RO when GainAuto is not Off (0 < gain & gain < 48)]]></ToolTip>
    <Address>0x20040</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>0.0</Min>
    <Max>48.0</Max>
    <Endianess>BigEndian</Endianess>
    <pIsLocked>GainAutoActive</pIsLocked>
  </Float>

  <Enumeration Name="GainAuto" NameSpace="Standard">
    <ToolTip>Automatic gain control</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Once"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Continuous"><Value>2</Value></EnumEntry>
    <pValue>GainAutoReg</pValue>
  </Enumeration>
  <IntReg Name="GainAutoReg"><Address>0x20048</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <IntSwissKnife Name="GainAutoActive">
    <Formula>GA &lt;&gt; 0</Formula>
    <pVariable Name="GA">GainAutoReg</pVariable>
  </IntSwissKnife>

  <Integer Name="BlackLevel" NameSpace="Standard">
    <ToolTip>Analog black level offset</ToolTip>
    <Address>0x20050</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <Max>255</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <!-- ════════════════════════════════════════════════════════════════════
       Transport Layer Control (Timestamp)
       ════════════════════════════════════════════════════════════════════ -->

  <Integer Name="GevTimestampTickFrequency" NameSpace="Standard">
    <ToolTip>Device timestamp tick frequency in Hz (1 GHz)</ToolTip>
    <Address>0x20060</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Integer Name="GevTimestampValue" NameSpace="Standard">
    <ToolTip>Current device timestamp in ticks (latched)</ToolTip>
    <Address>0x20068</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <IntReg Name="GevIEEE1588OffsetFromMasterLatched_Val" NameSpace="Custom">
    <ToolTip>PTP offset from master in ns, latched</ToolTip>
    <Address>0x20078</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>

  <IntReg Name="ChunkTimestamp_Val" NameSpace="Custom">
    <ToolTip>Chunk timestamp in ticks</ToolTip>
    <Address>0x20110</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <IntReg Name="ChunkWidth_Val" NameSpace="Custom">
    <ToolTip>Chunk image width in pixels</ToolTip>
    <Address>0x20118</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>LittleEndian</Endianess>
  </IntReg>

  <Command Name="TimestampLatch" NameSpace="Standard">
    <ToolTip>Latch the current timestamp into GevTimestampValue</ToolTip>
    <Address>0x20070</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

  <!-- ════════════════════════════════════════════════════════════════════
       Chunk Data Control
       ════════════════════════════════════════════════════════════════════ -->

  <!-- SFNC defines ChunkModeActive and ChunkEnable as IBoolean, and all 23
       vendor-corpus documents that declare ChunkModeActive use <Boolean> over
       a backing <IntReg> - none uses <Integer>. Declaring them as <Integer>
       here made Camera::configure_chunks, which correctly calls set_bool,
       fail against the only camera we can test with. -->
  <Boolean Name="ChunkModeActive" NameSpace="Standard">
    <ToolTip>Enable chunk data in image frames</ToolTip>
    <pValue>ChunkModeActiveReg</pValue>
  </Boolean>
  <IntReg Name="ChunkModeActiveReg"><Address>0x20080</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Enumeration Name="ChunkSelector" NameSpace="Standard">
    <ToolTip>Select which chunk feature to configure</ToolTip>
    <EnumEntry Name="Timestamp"><Value>1</Value></EnumEntry>
    <EnumEntry Name="ExposureTime"><Value>2</Value></EnumEntry>
    <EnumEntry Name="Gain"><Value>3</Value></EnumEntry>
    <pValue>ChunkSelectorReg</pValue>
  </Enumeration>
  <IntReg Name="ChunkSelectorReg"><Address>0x20084</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Boolean Name="ChunkEnable" NameSpace="Standard">
    <ToolTip>Enable the selected chunk feature</ToolTip>
    <pValue>ChunkEnableReg</pValue>
  </Boolean>
  <IntReg Name="ChunkEnableReg"><Address>0x20088</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- Event delivery is selected through GenApi, not through a bootstrap
       register: EventSelector picks a GigE Vision event id and
       EventNotification turns it on. The ids are the standard ones
       (GEV_EVENT_START_OF_TRANSFER = 0x0005, END_OF_TRANSFER = 0x0006). -->
  <Enumeration Name="EventSelector" NameSpace="Standard">
    <ToolTip>Select which event to configure</ToolTip>
    <EnumEntry Name="StartOfTransfer"><Value>5</Value></EnumEntry>
    <EnumEntry Name="EndOfTransfer"><Value>6</Value></EnumEntry>
    <pValue>EventSelectorReg</pValue>
  </Enumeration>
  <IntReg Name="EventSelectorReg"><Address>0x200A0</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Enumeration Name="EventNotification" NameSpace="Standard">
    <ToolTip>Enable notification for the selected event</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="On"><Value>1</Value></EnumEntry>
    <pValue>EventNotificationReg</pValue>
  </Enumeration>
  <IntReg Name="EventNotificationReg"><Address>0x200A4</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

</RegisterDescription>
"#;

// ── Register Map ────────────────────────────────────────────────────────────

/// Pre-populated register store for the fake camera.
///
/// All feature registers are initialized with realistic defaults.
/// The register map is thread-safe via external `Mutex` wrapping.
pub struct RegisterMap {
    regs: HashMap<u64, Vec<u8>>,
    xml_blob: Vec<u8>,
    clock_origin: Instant,
    /// Event identifiers whose `EventNotification` is `On`.
    ///
    /// One word cannot hold this. `EventSelector`/`EventNotification` is a
    /// selector pair, so a controller enabling two events writes the same
    /// address twice with a different selector in between — and a single
    /// stored word would let the second write turn the first event back off.
    enabled_events: HashSet<u16>,
    /// When a register-access command was last served, for the heartbeat rule.
    last_register_command: Instant,
    /// Whether [`RegisterMap::enforce_heartbeat`] is armed.
    enforce_heartbeat: bool,
    /// Largest `GevSCPSPacketSize` this device accepts, if it clamps at all.
    ///
    /// Real cameras cap the packet size at what their MAC can emit and reduce a
    /// larger request silently — the write is acknowledged and the register then
    /// reads back lower. Nothing distinguishes that from acceptance on the wire,
    /// which is why a host that trusts its own request reassembles at the wrong
    /// stride and completes no frame
    /// ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)).
    ///
    /// The fake accepted anything until 0.4.1, so it could not express the
    /// camera that caused that report, and no test could have caught the defect
    /// — the ADR-0019 failure mode of a fake that only agrees with its client.
    /// `None` keeps the old accept-anything behaviour.
    max_packet_size: Option<u32>,
    /// Largest GVSP datagram this device's *path* will actually deliver.
    ///
    /// Distinct from [`RegisterMap::max_packet_size`], and the distinction is
    /// the whole of [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112):
    /// that camera declares `Max=16366`, stores 16114 without complaint, and
    /// then streams nothing, because the link tops out at a 9216-byte frame.
    /// A register read cannot find that; only a test packet can.
    max_on_wire: Option<u32>,
    /// Size requested by the most recent write with the fire-test-packet bit,
    /// for the GVCP server to act on once it has released the register lock.
    pending_test_packet: Option<u32>,
}

/// `GevSCPSPacketSize` bit 31: send one test packet of the requested size.
pub const SCPS_FIRE_TEST_PACKET: u32 = 0x8000_0000;
/// `GevSCPSPacketSize` bit 30: set do-not-fragment on transmitted packets.
pub const SCPS_DO_NOT_FRAGMENT: u32 = 0x4000_0000;
/// The bits of `GevSCPSPacketSize` that hold the size itself.
pub const STREAM_PACKET_SIZE_MASK: u32 = 0xFFFF;

/// Compress the GenApi XML into a single-entry ZIP archive (deflate).
fn zip_xml_blob(xml: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("fake.xml", options)
        .expect("start XML zip entry");
    writer.write_all(xml).expect("write XML zip entry");
    writer.finish().expect("finish XML zip").into_inner()
}

impl RegisterMap {
    /// Create a new register map with the given image dimensions.
    ///
    /// Initializes all bootstrap, feature, and device info registers with
    /// sensible defaults. The GenApi XML is embedded at [`XML_BLOB_BASE`],
    /// served as a ZIP archive when `zip_xml` is set (as many real cameras
    /// do).
    pub fn new(width: u32, height: u32, pixel_format: u32, zip_xml: bool) -> Self {
        let mut regs = HashMap::new();

        // ── Bootstrap registers ─────────────────────────────────────────
        // The mandatory block every GigE Vision device answers. The fake used
        // to leave it at zero, which reads as "this camera reports version
        // 0.0 and has no stream channels" — a diagnostic dump of a real
        // camera would look nothing like one taken from the fake.
        regs.insert(VERSION, 0x0002_0000u32.to_be_bytes().to_vec()); // GEV 2.0
        regs.insert(DEVICE_MODE, DEVICE_MODE_VALUE.to_be_bytes().to_vec());
        regs.insert(
            DEVICE_MAC_HIGH,
            u32::from(u16::from_be_bytes([FAKE_MAC[0], FAKE_MAC[1]]))
                .to_be_bytes()
                .to_vec(),
        );
        regs.insert(
            DEVICE_MAC_LOW,
            u32::from_be_bytes([FAKE_MAC[2], FAKE_MAC[3], FAKE_MAC[4], FAKE_MAC[5]])
                .to_be_bytes()
                .to_vec(),
        );
        // Persistent IP + DHCP + link-local, matching CURRENT_IP_CONFIG below.
        regs.insert(SUPPORTED_IP_CONFIG, 0x8000_0007u32.to_be_bytes().to_vec());
        regs.insert(CURRENT_IP_ADDRESS, Ipv4Addr::LOCALHOST.octets().to_vec());
        regs.insert(CURRENT_SUBNET_MASK, [255, 0, 0, 0].to_vec());
        regs.insert(CURRENT_DEFAULT_GATEWAY, vec![0, 0, 0, 0]);
        regs.insert(NUMBER_OF_MESSAGE_CHANNELS, 1u32.to_be_bytes().to_vec());
        regs.insert(NUMBER_OF_STREAM_CHANNELS, 1u32.to_be_bytes().to_vec());
        regs.insert(CCP, vec![0, 0, 0, 0]);
        regs.insert(HEARTBEAT_TIMEOUT, 3000u32.to_be_bytes().to_vec());
        regs.insert(MESSAGE_CHANNEL_PORT, 0u32.to_be_bytes().to_vec());
        regs.insert(MESSAGE_CHANNEL_ADDRESS, vec![0, 0, 0, 0]);

        // IP configuration: DHCP + persistent + LLA = 0x07
        regs.insert(CURRENT_IP_CONFIG, 0x0000_0005u32.to_be_bytes().to_vec());
        regs.insert(PERSISTENT_IP_ADDRESS, vec![0, 0, 0, 0]);
        regs.insert(PERSISTENT_SUBNET_MASK, vec![0, 0, 0, 0]);
        regs.insert(PERSISTENT_DEFAULT_GATEWAY, vec![0, 0, 0, 0]);

        // Stream channel 0
        let base = STREAM_CHANNEL_BASE;
        regs.insert(base + SCP_HOST_PORT, vec![0, 0, 0, 0]);
        regs.insert(base + SCP_PACKET_SIZE, 1500u32.to_be_bytes().to_vec());
        // A second channel, so a `<pIndex>` term that is ignored is visible:
        // its packet size differs from channel 0's.
        regs.insert(
            base + STREAM_CHANNEL_STRIDE + SCP_PACKET_SIZE,
            9000u32.to_be_bytes().to_vec(),
        );
        regs.insert(REG_STREAM_CHANNEL_SELECTOR, 0u32.to_be_bytes().to_vec());
        regs.insert(base + SCP_PACKET_DELAY, vec![0, 0, 0, 0]);
        regs.insert(base + SCP_DEST_ADDR, vec![0, 0, 0, 0]);

        // ── Device info (read-only strings) ─────────────────────────────
        regs.insert(REG_DEVICE_MODEL_NAME, pad_string("VivaCam Fake", 32));
        regs.insert(REG_DEVICE_VENDOR_NAME, pad_string("vitavision.dev", 32));
        regs.insert(REG_DEVICE_SERIAL_NUMBER, pad_string("VIVA-FAKE-001", 16));
        regs.insert(REG_DEVICE_FIRMWARE_VERSION, pad_string("1.0.0-fake", 32));
        regs.insert(REG_DEVICE_ID, pad_string("VivaCam-0", 32));

        // ── Image format ────────────────────────────────────────────────
        regs.insert(REG_WIDTH, width.to_be_bytes().to_vec());
        regs.insert(REG_HEIGHT, height.to_be_bytes().to_vec());
        regs.insert(REG_PIXEL_FORMAT, pixel_format.to_be_bytes().to_vec());
        regs.insert(REG_OFFSET_X, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_OFFSET_Y, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_SENSOR_WIDTH, 4096u32.to_be_bytes().to_vec());
        regs.insert(REG_SENSOR_HEIGHT, 4096u32.to_be_bytes().to_vec());

        // Width/Height limits
        regs.insert(REG_WIDTH_MIN, 16u32.to_be_bytes().to_vec());
        regs.insert(REG_WIDTH_MAX, 4096u32.to_be_bytes().to_vec());
        regs.insert(REG_HEIGHT_MIN, 16u32.to_be_bytes().to_vec());
        regs.insert(REG_HEIGHT_MAX, 4096u32.to_be_bytes().to_vec());

        // ── Acquisition control ─────────────────────────────────────────
        regs.insert(REG_ACQ_MODE, 0u32.to_be_bytes().to_vec()); // Continuous
        regs.insert(REG_ACQ_START, vec![0, 0, 0, 0]);
        regs.insert(REG_USER_SET_SELECTOR, 0u32.to_be_bytes().to_vec()); // Default
        regs.insert(REG_USER_SET_LOAD, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_SOFTWARE_SIGNAL_PULSE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_ACQ_STOP, vec![0, 0, 0, 0]);
        regs.insert(REG_ACQ_FRAME_RATE, 30.0f32.to_be_bytes().to_vec());
        regs.insert(
            REG_EXPOSURE_TIME,
            DEFAULT_EXPOSURE_US.to_be_bytes().to_vec(),
        );
        regs.insert(REG_EXPOSURE_AUTO, 0u32.to_be_bytes().to_vec()); // Off

        // ── Analog control ──────────────────────────────────────────────
        regs.insert(REG_GAIN, 0.0f64.to_be_bytes().to_vec());
        regs.insert(REG_GAIN_AUTO, 0u32.to_be_bytes().to_vec()); // Off
        regs.insert(REG_BLACK_LEVEL, 0u32.to_be_bytes().to_vec());

        // ── Predicate gating ────────────────────────────────────────────
        // Frame rate manually controllable by default; sensor boots as a
        // monochrome sensor so Mono8/Mono16 PixelFormat entries are
        // available at boot.
        regs.insert(REG_ACQ_FRAME_RATE_ENABLE, 1u32.to_be_bytes().to_vec());
        regs.insert(REG_SENSOR_TYPE, 0u32.to_be_bytes().to_vec());
        // Capability bits, MSB-first as GenICam counts them on a big-endian
        // register: bit 0 = frame rate control present, bit 1 = chunk support.
        regs.insert(REG_DEVICE_CAPS, 0xC000_0000u32.to_be_bytes().to_vec());
        // ExposureTime implemented (bit 0) and available (bit 1), MSB-first —
        // see REG_FEATURE_STATUS.
        regs.insert(REG_FEATURE_STATUS, 0xC000_0000u32.to_be_bytes().to_vec());

        // ── Timestamp (1 GHz tick frequency) ────────────────────────────
        regs.insert(REG_TIMESTAMP_FREQ, 1_000_000_000u32.to_be_bytes().to_vec());
        regs.insert(REG_TIMESTAMP_VALUE, vec![0u8; 8]);
        regs.insert(REG_TIMESTAMP_LATCH, vec![0, 0, 0, 0]);
        regs.insert(
            REG_PTP_OFFSET_LATCHED,
            (PTP_OFFSET_NS as u64).to_be_bytes().to_vec(),
        );
        regs.insert(
            REG_CHUNK_TIMESTAMP_LE,
            CHUNK_TIMESTAMP_TICKS.to_le_bytes().to_vec(),
        );
        regs.insert(REG_CHUNK_WIDTH_LE, CHUNK_WIDTH_PX.to_le_bytes().to_vec());

        // ── Chunk data ──────────────────────────────────────────────────
        regs.insert(REG_CHUNK_MODE_ACTIVE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_CHUNK_SELECTOR, 1u32.to_be_bytes().to_vec()); // Timestamp
        regs.insert(REG_CHUNK_ENABLE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_EVENT_SELECTOR, 5u32.to_be_bytes().to_vec());

        // ── XML URL register ────────────────────────────────────────────
        let (xml_blob, xml_name) = if zip_xml {
            (zip_xml_blob(FAKE_XML.as_bytes()), "fake.zip")
        } else {
            (FAKE_XML.as_bytes().to_vec(), "fake.xml")
        };
        let url = format!(
            "Local:{xml_name};{:x};{:x}\0",
            XML_BLOB_BASE,
            xml_blob.len()
        );
        let mut url_bytes = vec![0u8; URL_REG_LEN];
        let src = url.as_bytes();
        url_bytes[..src.len()].copy_from_slice(src);
        regs.insert(FIRST_URL_REG, url_bytes);

        Self {
            regs,
            xml_blob,
            clock_origin: Instant::now(),
            enabled_events: HashSet::new(),
            last_register_command: Instant::now(),
            enforce_heartbeat: false,
            max_packet_size: None,
            max_on_wire: None,
            pending_test_packet: None,
        }
    }

    /// Arm the GigE Vision heartbeat rule: release control privilege when the
    /// controller goes quiet for longer than `GevHeartbeatTimeout`.
    ///
    /// Real devices always do this, and it is the entire reason a client needs a
    /// keepalive — GVSP image traffic does not refresh the timer, so a camera can
    /// be streaming at full rate while its control channel times out underneath.
    /// A fake that never expires CCP cannot tell a working keepalive from a
    /// missing one, which is how SR-05 stayed open through three app-layer
    /// reimplementations of the same loop.
    ///
    /// **Off by default**, because arming it makes every test that holds control
    /// privilege sensitive to a 3 s stall — on a loaded CI runner that is a
    /// flake, not a finding. Tests that are *about* the keepalive turn it on.
    pub fn enforce_heartbeat(&mut self, enable: bool) {
        self.enforce_heartbeat = enable;
        self.last_register_command = Instant::now();
    }

    /// Clamp `GevSCPSPacketSize` writes to `max`, the way a real device does.
    ///
    /// A request above `max` is acknowledged and silently reduced, so the
    /// register reads back lower than what was written — nothing on the wire
    /// distinguishes that from acceptance, which is the defect in
    /// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112).
    /// Silently drop any GVSP datagram larger than `max`, as a path with a
    /// smaller frame ceiling than either endpoint believes does.
    ///
    /// See [`RegisterMap::max_on_wire`].
    pub fn set_max_on_wire(&mut self, max: u32) {
        self.max_on_wire = Some(max);
    }

    /// The path ceiling, if one was configured.
    pub fn max_on_wire(&self) -> Option<u32> {
        self.max_on_wire
    }

    /// Take the size requested by the last fire-test-packet write, if any.
    pub fn take_pending_test_packet(&mut self) -> Option<u32> {
        self.pending_test_packet.take()
    }

    pub fn set_max_packet_size(&mut self, max: u32) {
        self.max_packet_size = Some(max);
        // Apply it to whatever the register already holds, so a device
        // configured with a cap never reports a size above it.
        let current = self.stream_packet_size();
        if current > max {
            self.write(STREAM_CHANNEL_BASE + SCP_PACKET_SIZE, &max.to_be_bytes());
        }
    }

    /// Apply the heartbeat rule, and report whether privilege was just revoked.
    ///
    /// Called for register-access commands only. Discovery and FORCEIP are
    /// broadcast by any application on the subnet, so counting them would let an
    /// unrelated `viva-camctl list` hold another application's privilege open.
    /// Unlike a real device we do not track *which* peer is the controller —
    /// there is only ever one in a test.
    pub fn note_register_command(&mut self) -> bool {
        let elapsed = self.last_register_command.elapsed();
        self.last_register_command = Instant::now();
        if !self.enforce_heartbeat {
            return false;
        }
        let timeout = Duration::from_millis(u64::from(self.heartbeat_timeout_ms()));
        if timeout.is_zero() || elapsed <= timeout {
            return false;
        }
        let ccp = self.read(CCP, 4);
        if u32::from_be_bytes([ccp[0], ccp[1], ccp[2], ccp[3]]) == 0 {
            return false;
        }
        self.write(CCP, &0u32.to_be_bytes());
        true
    }

    /// Report a different `GevHeartbeatTimeout` than the 3 000 ms default.
    ///
    /// Lets a test pick a window short enough to wait out without making the
    /// suite slow, and makes the timing it depends on explicit rather than
    /// implied by this crate's default.
    pub fn set_heartbeat_timeout_ms(&mut self, timeout_ms: u32) {
        self.write(HEARTBEAT_TIMEOUT, &timeout_ms.to_be_bytes());
    }

    /// Report `mac` from the bootstrap MAC registers instead of [`FAKE_MAC`],
    /// so they agree with a Discovery ACK built from the same identity.
    pub fn set_mac(&mut self, mac: [u8; 6]) {
        let high = u32::from(u16::from_be_bytes([mac[0], mac[1]]));
        let low = u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]);
        self.regs
            .insert(DEVICE_MAC_HIGH, high.to_be_bytes().to_vec());
        self.regs.insert(DEVICE_MAC_LOW, low.to_be_bytes().to_vec());
    }

    /// The heartbeat window this device reports, in milliseconds.
    pub fn heartbeat_timeout_ms(&self) -> u32 {
        let data = self.read(HEARTBEAT_TIMEOUT, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Whether `[addr, addr + len)` touches a register the XML declares `WO`.
    ///
    /// Checked by the GVCP server before it serves a read, so a READREG or
    /// READMEM of a write-only register is refused on the wire. The store
    /// itself stays readable: `peek`-style test access and the fake's own
    /// side effects must still see what was written.
    pub fn is_write_only(&self, addr: u64, len: usize) -> bool {
        let end = addr.saturating_add(len as u64);
        WRITE_ONLY_REGISTERS
            .iter()
            .any(|&reg| reg < end && addr < reg + 4)
    }

    /// Read `len` bytes starting at `addr`.
    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        // XML blob region
        if addr >= XML_BLOB_BASE {
            let offset = (addr - XML_BLOB_BASE) as usize;
            if offset < self.xml_blob.len() {
                let end = (offset + len).min(self.xml_blob.len());
                let mut result = self.xml_blob[offset..end].to_vec();
                result.resize(len, 0);
                return result;
            }
        }

        // `EventNotification` reports the selected event, not a stored word.
        if addr == REG_EVENT_NOTIFICATION {
            let on = u32::from(self.event_notification_on(self.event_selector()));
            let mut result = on.to_be_bytes().to_vec();
            result.resize(len, 0);
            result.truncate(len);
            return result;
        }

        // Exact register match
        if let Some(data) = self.regs.get(&addr) {
            let mut result = data.clone();
            result.resize(len, 0);
            result.truncate(len);
            return result;
        }

        // Sub-register access (read within a larger register)
        for (&reg_addr, data) in &self.regs {
            if addr >= reg_addr && (addr - reg_addr) < data.len() as u64 {
                let offset = (addr - reg_addr) as usize;
                let end = (offset + len).min(data.len());
                let mut result = data[offset..end].to_vec();
                result.resize(len, 0);
                return result;
            }
        }

        vec![0u8; len]
    }

    /// Write `data` starting at `addr`.
    pub fn write(&mut self, addr: u64, data: &[u8]) {
        // `GevSCPSPacketSize` carries two flags above the 16-bit size field:
        // bit 31 fires a test packet and bit 30 sets do-not-fragment. Neither
        // is part of the stored value — a device that echoed them back would
        // report a packet size of over a billion — so they are recorded for
        // the caller and stripped here.
        let stripped;
        let mut data = data;
        if addr == STREAM_CHANNEL_BASE + SCP_PACKET_SIZE && data.len() >= 4 {
            let raw = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            self.pending_test_packet =
                (raw & SCPS_FIRE_TEST_PACKET != 0).then_some(raw & STREAM_PACKET_SIZE_MASK);
            if raw & (SCPS_FIRE_TEST_PACKET | SCPS_DO_NOT_FRAGMENT) != 0 {
                stripped = (raw & STREAM_PACKET_SIZE_MASK).to_be_bytes();
                data = &stripped[..];
            }
        }

        // A capped device reduces an oversized packet size instead of refusing
        // it — silently, exactly as the hardware in #112 does.
        let clamped;
        let data = match self.max_packet_size {
            Some(max)
                if addr == STREAM_CHANNEL_BASE + SCP_PACKET_SIZE
                    && data.len() >= 4
                    && u32::from_be_bytes([data[0], data[1], data[2], data[3]]) > max =>
            {
                clamped = max.to_be_bytes();
                &clamped[..]
            }
            _ => data,
        };

        // `EventNotification` applies to the selected event. Enabling a second
        // event must not disable the first, so the value lands in the set
        // keyed by the current selector rather than in a shared register.
        if addr == REG_EVENT_NOTIFICATION && data.len() >= 4 {
            let event = self.event_selector();
            if u32::from_be_bytes([data[0], data[1], data[2], data[3]]) != 0 {
                self.enabled_events.insert(event);
            } else {
                self.enabled_events.remove(&event);
            }
            return;
        }

        if let Some(existing) = self.regs.get_mut(&addr) {
            let len = existing.len().min(data.len());
            existing[..len].copy_from_slice(&data[..len]);
            return;
        }

        // Write within an existing register
        let addrs: Vec<u64> = self.regs.keys().copied().collect();
        for reg_addr in addrs {
            let reg_data = self.regs.get(&reg_addr).unwrap();
            if addr >= reg_addr && (addr - reg_addr) < reg_data.len() as u64 {
                let offset = (addr - reg_addr) as usize;
                let end = (offset + data.len()).min(reg_data.len());
                let reg_data = self.regs.get_mut(&reg_addr).unwrap();
                reg_data[offset..end].copy_from_slice(&data[..end - offset]);
                return;
            }
        }

        self.regs.insert(addr, data.to_vec());
    }

    /// Handle side effects of register writes.
    pub fn handle_special_write(&mut self, addr: u64) {
        if addr == REG_TIMESTAMP_LATCH {
            let ts = self.device_timestamp();
            self.regs
                .insert(REG_TIMESTAMP_VALUE, ts.to_be_bytes().to_vec());
        }
        if addr == REG_USER_SET_LOAD {
            self.load_user_set();
        }
    }

    /// Restore the analog-control defaults, as `UserSetLoad` does on a real
    /// camera.
    ///
    /// A command that only acknowledges the write is untestable: a test could
    /// assert nothing beyond the absence of an error, which is the fake
    /// agreeing with itself. Restoring observable device state means a test can
    /// change a feature, execute the command, and read the change back out
    /// (ADR-0019).
    fn load_user_set(&mut self) {
        self.regs.insert(
            REG_EXPOSURE_TIME,
            DEFAULT_EXPOSURE_US.to_be_bytes().to_vec(),
        );
        self.regs.insert(REG_GAIN, 0.0f64.to_be_bytes().to_vec());
        self.regs
            .insert(REG_EXPOSURE_AUTO, 0u32.to_be_bytes().to_vec());
        self.regs.insert(REG_GAIN_AUTO, 0u32.to_be_bytes().to_vec());
    }

    // ── Accessors ───────────────────────────────────────────────────────

    /// Current device timestamp in nanoseconds since creation.
    pub fn device_timestamp(&self) -> u64 {
        self.clock_origin.elapsed().as_nanos() as u64
    }

    /// Stream destination IP address.
    pub fn stream_dest_ip(&self) -> Ipv4Addr {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_DEST_ADDR, 4);
        Ipv4Addr::new(data[0], data[1], data[2], data[3])
    }

    /// Stream destination port.
    pub fn stream_dest_port(&self) -> u16 {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_HOST_PORT, 4);
        u16::from_be_bytes([data[2], data[3]])
    }

    /// Message channel destination address (`GevMCDA`).
    pub fn message_dest_ip(&self) -> Ipv4Addr {
        let data = self.read(MESSAGE_CHANNEL_ADDRESS, 4);
        Ipv4Addr::new(data[0], data[1], data[2], data[3])
    }

    /// Message channel destination port (`GevMCP`), low 16 bits of the register.
    pub fn message_dest_port(&self) -> u16 {
        let data = self.read(MESSAGE_CHANNEL_PORT, 4);
        u16::from_be_bytes([data[2], data[3]])
    }

    /// Event identifier currently selected by `EventSelector`.
    pub fn event_selector(&self) -> u16 {
        let data = self.read(REG_EVENT_SELECTOR, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u16
    }

    /// Whether `EventNotification` is `On` for `event_id`.
    ///
    /// Takes the event explicitly: the emitter cares whether *its* event is
    /// enabled, which is independent of whichever entry the controller
    /// happened to select last.
    pub fn event_notification_on(&self, event_id: u16) -> bool {
        self.enabled_events.contains(&event_id)
    }

    /// Stream packet size.
    pub fn stream_packet_size(&self) -> u32 {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_PACKET_SIZE, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Image width.
    pub fn width(&self) -> u32 {
        let data = self.read(REG_WIDTH, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Image height.
    pub fn height(&self) -> u32 {
        let data = self.read(REG_HEIGHT, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Pixel format PFNC code.
    pub fn pixel_format_code(&self) -> u32 {
        let data = self.read(REG_PIXEL_FORMAT, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Whether chunk mode is active.
    pub fn chunk_mode_active(&self) -> bool {
        let data = self.read(REG_CHUNK_MODE_ACTIVE, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]]) != 0
    }

    /// Current exposure time in microseconds.
    pub fn exposure_time(&self) -> f64 {
        let data = self.read(REG_EXPOSURE_TIME, 8);
        f64::from_be_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
        ])
    }
}

/// Pad a string to a fixed length with null bytes.
fn pad_string(s: &str, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let src = s.as_bytes();
    let copy_len = src.len().min(len);
    buf[..copy_len].copy_from_slice(&src[..copy_len]);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> RegisterMap {
        RegisterMap::new(64, 48, 0x0108_0001, false)
    }

    fn select_and_enable(regs: &mut RegisterMap, event: u32, on: bool) {
        regs.write(REG_EVENT_SELECTOR, &event.to_be_bytes());
        regs.write(REG_EVENT_NOTIFICATION, &u32::from(on).to_be_bytes());
    }

    /// `EventNotification` is a selected feature, so a controller enabling two
    /// events writes the same address twice. Backing it with a single stored
    /// word made the second write turn the first event off, and the fake then
    /// silently emitted nothing — the failure a fake exists to prevent.
    #[test]
    fn enabling_a_second_event_leaves_the_first_enabled() {
        let mut regs = map();
        select_and_enable(&mut regs, 5, true);
        select_and_enable(&mut regs, 6, true);
        assert!(regs.event_notification_on(5));
        assert!(regs.event_notification_on(6));
    }

    #[test]
    fn disabling_one_event_leaves_the_others_alone() {
        let mut regs = map();
        select_and_enable(&mut regs, 5, true);
        select_and_enable(&mut regs, 6, true);
        select_and_enable(&mut regs, 5, false);
        assert!(!regs.event_notification_on(5));
        assert!(regs.event_notification_on(6));
    }

    /// The MAC in the bootstrap registers must be the MAC in the Discovery
    /// ACK. Two independent copies of one fact is how #57 happened: the ACK
    /// layout drifted and nothing compared it against anything.
    #[test]
    fn the_bootstrap_mac_matches_the_discovery_mac() {
        let regs = map();
        let high = regs.read(DEVICE_MAC_HIGH, 4);
        let low = regs.read(DEVICE_MAC_LOW, 4);
        assert_eq!(&high[..2], &[0, 0], "top two bytes are reserved");
        let mac = [high[2], high[3], low[0], low[1], low[2], low[3]];
        assert_eq!(mac, FAKE_MAC);
    }

    #[test]
    fn set_mac_rewrites_the_bootstrap_mac() {
        let mut regs = map();
        let other = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
        regs.set_mac(other);
        let high = regs.read(DEVICE_MAC_HIGH, 4);
        let low = regs.read(DEVICE_MAC_LOW, 4);
        assert_eq!(&high[..2], &[0, 0], "top two bytes are reserved");
        assert_eq!([high[2], high[3], low[0], low[1], low[2], low[3]], other);
    }

    #[test]
    fn the_device_reports_one_channel_of_each_kind() {
        let regs = map();
        assert_eq!(regs.read(NUMBER_OF_MESSAGE_CHANNELS, 4), vec![0, 0, 0, 1]);
        assert_eq!(regs.read(NUMBER_OF_STREAM_CHANNELS, 4), vec![0, 0, 0, 1]);
    }

    #[test]
    fn no_event_is_enabled_by_default() {
        let regs = map();
        assert!(!regs.event_notification_on(5));
        assert!(!regs.event_notification_on(6));
    }

    /// Reading the register back must report the *selected* event, which is
    /// what a GenApi `EventNotification` read does.
    #[test]
    fn reading_notification_follows_the_selector() {
        let mut regs = map();
        select_and_enable(&mut regs, 5, true);
        regs.write(REG_EVENT_SELECTOR, &6u32.to_be_bytes());
        assert_eq!(regs.read(REG_EVENT_NOTIFICATION, 4), vec![0, 0, 0, 0]);
        regs.write(REG_EVENT_SELECTOR, &5u32.to_be_bytes());
        assert_eq!(regs.read(REG_EVENT_NOTIFICATION, 4), vec![0, 0, 0, 1]);
    }
}
