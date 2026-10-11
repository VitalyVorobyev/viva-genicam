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
//! | `0x0934`    | 4      | GVCP Capability (RO)            | u32 BE   |
//! | `0x0938`    | 4      | Heartbeat Timeout               | u32 BE   |
//! | `0x093c`    | 4      | Timestamp tick frequency, high  | u32 BE   |
//! | `0x0940`    | 4      | Timestamp tick frequency, low   | u32 BE   |
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
//! | `0x200ac`   | 4      | UserSetLoad (command)           | u32 BE   |
//! | `0x200b0`   | 4      | SoftwareSignal*Pulse (WO bits)  | u32 BE   |
//! | `0x200b4`   | 4      | UserSetSave (command)           | u32 BE   |
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
use std::net::{Ipv4Addr, SocketAddr};
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
/// `GVCP Capability`: which optional commands the device implements.
pub const GVCP_CAPABILITY: u64 = 0x0934;
/// Capability bit 29, counted from the MSB as bit 0: `PACKETRESEND_CMD`
/// supported. Mask from Wireshark's `hf_gvcp_capability_packetresend`.
pub const CAPABILITY_PACKET_RESEND: u32 = 0x0000_0004;
/// Capability bit 30: `WRITEMEM_CMD` supported.
pub const CAPABILITY_WRITEMEM: u32 = 0x0000_0002;
/// Capability bit 31: several registers in one READREG/WRITEREG.
pub const CAPABILITY_CONCATENATION: u32 = 0x0000_0001;
/// Capability bit 26: the device can answer a slow command with `PENDING_ACK`.
/// Mask from Wireshark's `hf_gvcp_capability_pending`.
pub const CAPABILITY_PENDING_ACK: u32 = 0x0000_0020;
/// `GVCP Configuration`: what the controller has switched on.
pub const GVCP_CONFIGURATION: u64 = 0x0954;
/// Configuration bit 30: `PENDING_ACK` enabled. A device sends one only once
/// the controller has set it. Mask from Wireshark's
/// `hf_gvcp_configuration_pending_ack_enable`.
pub const CONFIGURATION_PENDING_ACK_ENABLE: u32 = 0x0000_0002;
/// Capability bit 4: the primary application's port and IP are readable at
/// [`PRIMARY_APPLICATION_PORT`] and [`PRIMARY_APPLICATION_IP`]. Mask from
/// Wireshark's `hf_gvcp_capability_ccp_application_portip`.
pub const CAPABILITY_CCP_APPLICATION_PORT_IP: u32 = 0x0800_0000;
/// The UDP port of the application holding control, in the low half; 0 while
/// nobody holds it.
pub const PRIMARY_APPLICATION_PORT: u64 = 0x0A04;
/// The IPv4 address of the application holding control; 0 while nobody does.
pub const PRIMARY_APPLICATION_IP: u64 = 0x0A14;
/// CCP bit 31: exclusive access. No other application may read or write.
pub const CCP_EXCLUSIVE_ACCESS: u32 = 0x0000_0001;
/// CCP bit 30: control access. Other applications may still read.
pub const CCP_CONTROL_ACCESS: u32 = 0x0000_0002;
pub const HEARTBEAT_TIMEOUT: u64 = 0x0938;
/// `GevTimestampTickFrequencyHigh`: upper 32 bits of the tick frequency.
pub const TIMESTAMP_TICK_FREQUENCY_HIGH: u64 = 0x093C;
/// `GevTimestampTickFrequencyLow`: lower 32 bits of the tick frequency.
pub const TIMESTAMP_TICK_FREQUENCY_LOW: u64 = 0x0940;
/// The fake's timestamp clock: nanoseconds, so 1 GHz.
pub const TICK_FREQUENCY_HZ: u64 = 1_000_000_000;
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
/// `ChunkEnable` for the selected chunk: a *selected* feature, backed by the
/// set of enabled selector values — see [`RegisterMap::chunk_enabled`].
pub const REG_CHUNK_ENABLE: u64 = 0x20088;

/// `ChunkSelector` entry values, this fake's own numbering.
pub const CHUNK_SELECTOR_TIMESTAMP: u32 = 1;
/// `ChunkSelector` = `ExposureTime`.
pub const CHUNK_SELECTOR_EXPOSURE_TIME: u32 = 2;
/// `ChunkSelector` = `FrameID`.
pub const CHUNK_SELECTOR_FRAME_ID: u32 = 4;
/// `ChunkSelector` = `ImageInfo`: width, height and image length in one
/// chunk, with the length at a negative address as Baumer's HXG20 declares
/// `ChunkImageLength`.
pub const CHUNK_SELECTOR_IMAGE_INFO: u32 = 5;

/// Chunk IDs on this fake's chunk ports, as its XML declares them. A real
/// camera's are whatever its XML says; two here are above 0xFFFF, which a
/// 16-bit ID could not carry.
pub const CHUNK_ID_IMAGE: u32 = 0x0000_0001;
/// `ChunkTimestamp`: the leader's timestamp, 8 bytes little-endian.
pub const CHUNK_ID_TIMESTAMP: u32 = 0x0000_0002;
/// `ChunkExposureTime`: IEEE 754 double, little-endian.
pub const CHUNK_ID_EXPOSURE_TIME: u32 = 0x0000_0003;
/// `ChunkFrameID`: the block id, 8 bytes little-endian.
pub const CHUNK_ID_FRAME_ID: u32 = 0x0504_0003;
/// `ImageInfo`: width, height, reserved, image length — four big-endian
/// `u32`s; `ChunkImageLength` reads the last at address `-4`.
pub const CHUNK_ID_IMAGE_INFO: u32 = 0xA5A5_0100;

/// GigE Vision event identifier this fake sends for ExposureEnd, as an
/// `EVENTDATA` carrying the frame id. Device-specific range, this fake's own
/// choice; its event port declares `<EventID>9001</EventID>`.
pub const EVENT_EXPOSURE_END: u16 = 0x9001;

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

/// Exposure the fake boots with, and returns to when `UserSetLoad` loads
/// `Default`.
pub const DEFAULT_EXPOSURE_US: f64 = 5000.0;

/// `REG_USER_SET_SELECTOR`, `REG_USER_SET_LOAD` and `REG_USER_SET_SAVE` back
/// the SFNC user-set features, so the fake can answer the workflow issue #121
/// was filed about.
///
/// They are shaped after the Hikrobot MV-CS050-10GC
/// (`Hikrobot_MV-CS050-10GC.xml:23009-23148` in the vendor corpus): both
/// commands take the value they write from `<pCommandValue>`, an
/// `<IntSwissKnife>` reading the selector register, so the *written value*
/// names the user set. The fake acts on that value, not on the selector
/// register — so a client that writes a constant `1` with `UserSet2` selected
/// saves into set 1 here as it would on the camera, and a test can see it
/// (backlog GA-35; ADR-0019's "fakes must be able to disagree").
pub const REG_USER_SET_SELECTOR: u64 = 0x200a8;
/// `UserSetLoad`: writing `n` loads user set `n`, `0` being `Default`.
pub const REG_USER_SET_LOAD: u64 = 0x200ac;
/// `UserSetSave`: writing `n` saves the current settings into user set `n`.
/// `Default` cannot be saved: the XML makes the command unavailable while it
/// is selected, and the fake ignores a write of `0`.
pub const REG_USER_SET_SAVE: u64 = 0x200b4;

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

/// `TriggerSelector`: which trigger `TriggerMode`, `TriggerSource` and
/// `TriggerSoftware` act on. `FrameStart` is 0, `AcquisitionStart` 1; the
/// selected value indexes the mode and source registers (`<pIndex>`, as the
/// Basler acA1600 and Hikrobot documents address theirs).
pub const REG_TRIGGER_SELECTOR: u64 = 0x200c0;
/// `TriggerSelector` value of `FrameStart`.
pub const TRIGGER_FRAME_START: u32 = 0;
/// `TriggerMode` of the trigger at index `n` lives at `REG_TRIGGER_MODE + 4n`;
/// `1` is `On`.
pub const REG_TRIGGER_MODE: u64 = 0x200c4;
/// `TriggerSource` of the trigger at index `n` lives at
/// `REG_TRIGGER_SOURCE + 4n`.
pub const REG_TRIGGER_SOURCE: u64 = 0x200d0;
/// `TriggerSource` value of `Line1`, the source the fake boots with.
pub const TRIGGER_SOURCE_LINE1: u32 = 0;
/// `TriggerSource` value of `Software`.
pub const TRIGGER_SOURCE_SOFTWARE: u32 = 1;
/// `TriggerSoftware`: a write starts one frame while the `FrameStart`
/// trigger is `On` with source `Software`. With the trigger on, the fake
/// sends no frame without one: the data path is gated, not only the register.
pub const REG_TRIGGER_SOFTWARE: u64 = 0x200e0;
/// `BinningHorizontal`, 1 to 4.
pub const REG_BINNING_HORIZONTAL: u64 = 0x200e4;
/// `BinningVertical`, 1 to 4.
pub const REG_BINNING_VERTICAL: u64 = 0x200e8;
/// `UserSetDefault` (SFNC 2) or, in the legacy profile,
/// `UserSetDefaultSelector`: the set the camera loads at power-up.
pub const REG_USER_SET_DEFAULT: u64 = 0x200ec;
/// `GainRaw` of `GainSelector` entry `n` lives at `REG_GAIN_RAW + 4n`
/// (legacy profile only).
pub const REG_GAIN_RAW: u64 = 0x200f0;

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
    REG_USER_SET_SAVE,
    REG_TRIGGER_SOFTWARE,
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
    <pFeature>UserSetSave</pFeature>
    <!-- profile:user-set-default-category --><pFeature>UserSetDefault</pFeature><!-- /profile:user-set-default-category -->
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
    <pFeature>BinningHorizontal</pFeature>
    <pFeature>BinningVertical</pFeature>
    <pFeature>PixelFormat</pFeature>
  </Category>

  <Category Name="AcquisitionControl">
    <DisplayName>Acquisition Control</DisplayName>
    <pFeature>AcquisitionMode</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
    <pFeature>AcquisitionFrameRateEnable</pFeature>
    <!-- profile:frame-rate-category --><pFeature>AcquisitionFrameRate</pFeature><!-- /profile:frame-rate-category -->
    <!-- profile:exposure-category --><pFeature>ExposureTime</pFeature><!-- /profile:exposure-category -->
    <pFeature>ExposureAuto</pFeature>
    <pFeature>TriggerSelector</pFeature>
    <pFeature>TriggerMode</pFeature>
    <pFeature>TriggerSource</pFeature>
    <pFeature>TriggerSoftware</pFeature>
  </Category>

  <Category Name="AnalogControl">
    <DisplayName>Analog Control</DisplayName>
    <!-- profile:gain-category --><pFeature>Gain</pFeature><!-- /profile:gain-category -->
    <pFeature>GainAuto</pFeature>
    <pFeature>BlackLevel</pFeature>
  </Category>

  <Category Name="TransportLayerControl">
    <DisplayName>Transport Layer Control</DisplayName>
    <pFeature>TLParamsLocked</pFeature>
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
    <pFeature>ChunkTimestamp</pFeature>
    <pFeature>ChunkExposureTime</pFeature>
    <pFeature>ChunkFrameID</pFeature>
    <pFeature>ChunkWidth</pFeature>
    <pFeature>ChunkHeight</pFeature>
    <pFeature>ChunkImageLength</pFeature>
  </Category>

  <Category Name="EventControl">
    <DisplayName>Event Control</DisplayName>
    <pFeature>EventSelector</pFeature>
    <pFeature>EventNotification</pFeature>
    <pFeature>EventExposureEnd</pFeature>
    <pFeature>EventExposureEndTimestamp</pFeature>
    <pFeature>EventExposureEndFrameID</pFeature>
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

  <!-- The ROI shape every corpus document shares: a size's maximum shrinks
       as its offset grows, and an offset's as the size grows, so the order
       of writes decides whether a new ROI can be reached at all. The size and
       format are locked while TLParamsLocked is set (FLIR, Basler, Hikrobot). -->
  <Integer Name="Width" NameSpace="Standard">
    <ToolTip>Width of the image in pixels</ToolTip>
    <Address>0x20000</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pIsLocked>TLParamsLocked</pIsLocked>
    <pMin>WidthMin</pMin>
    <pMax>WidthMaxNow</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <IntSwissKnife Name="WidthMaxNow">
    <pVariable Name="WMAX">WidthMax</pVariable>
    <pVariable Name="OX">OffsetX</pVariable>
    <Formula>WMAX - OX</Formula>
  </IntSwissKnife>
  <IntReg Name="WidthMin"><Address>0x20100</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <IntReg Name="WidthMax"><Address>0x20104</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="Height" NameSpace="Standard">
    <ToolTip>Height of the image in pixels</ToolTip>
    <Address>0x20004</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pIsLocked>TLParamsLocked</pIsLocked>
    <pMin>HeightMin</pMin>
    <pMax>HeightMaxNow</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <IntSwissKnife Name="HeightMaxNow">
    <pVariable Name="HMAX">HeightMax</pVariable>
    <pVariable Name="OY">OffsetY</pVariable>
    <Formula>HMAX - OY</Formula>
  </IntSwissKnife>
  <IntReg Name="HeightMin"><Address>0x20108</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <IntReg Name="HeightMax"><Address>0x2010c</Address><Length>4</Length><AccessMode>RO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="OffsetX" NameSpace="Standard">
    <ToolTip>Horizontal offset from the sensor origin</ToolTip>
    <Address>0x2000c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <pMax>OffsetXMax</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Integer Name="OffsetY" NameSpace="Standard">
    <ToolTip>Vertical offset from the sensor origin</ToolTip>
    <Address>0x20010</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <pMax>OffsetYMax</pMax>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <IntSwissKnife Name="OffsetXMax">
    <pVariable Name="WMAX">WidthMax</pVariable>
    <pVariable Name="W">Width</pVariable>
    <Formula>WMAX - W</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="OffsetYMax">
    <pVariable Name="HMAX">HeightMax</pVariable>
    <pVariable Name="H">Height</pVariable>
    <Formula>HMAX - H</Formula>
  </IntSwissKnife>

  <Integer Name="BinningHorizontal" NameSpace="Standard">
    <ToolTip>Pixels combined horizontally</ToolTip>
    <Address>0x200e4</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1</Min>
    <Max>4</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>
  <Integer Name="BinningVertical" NameSpace="Standard">
    <ToolTip>Pixels combined vertically</ToolTip>
    <Address>0x200e8</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1</Min>
    <Max>4</Max>
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
    <pIsLocked>TLParamsLocked</pIsLocked>
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

  <!-- Every <Command> here reaches its register through <pValue>, the shape
       all 490 commands in the vendor corpus use; none has an <Address> of its
       own (backlog GA-10). -->
  <Command Name="AcquisitionStart" NameSpace="Standard">
    <ToolTip>Start image acquisition</ToolTip>
    <pValue>AcquisitionStartReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="AcquisitionStartReg"><Address>0x20024</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- The Hikrobot MV-CS050-10GC user-set block (Hikrobot_MV-CS050-10GC.xml,
       lines 23009-23148): both commands write the selected set's index,
       taken through <pCommandValue> from the selector register, and saving is
       unavailable while Default is selected. -->
  <Enumeration Name="UserSetSelector" NameSpace="Standard">
    <ToolTip>User set that UserSetLoad and UserSetSave act on</ToolTip>
    <EnumEntry Name="Default"><Value>0</Value></EnumEntry>
    <EnumEntry Name="UserSet1"><Value>1</Value></EnumEntry>
    <EnumEntry Name="UserSet2"><Value>2</Value></EnumEntry>
    <pValue>UserSetSelectorReg</pValue>
    <pSelected>UserSetLoad</pSelected>
    <pSelected>UserSetSave</pSelected>
  </Enumeration>
  <IntReg Name="UserSetSelectorReg"><Address>0x200a8</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Command Name="UserSetLoad" NameSpace="Standard">
    <ToolTip>Load the selected user set</ToolTip>
    <ImposedAccessMode>WO</ImposedAccessMode>
    <pValue>UserSetLoadReg</pValue>
    <pCommandValue>UserSetLoad_CmdValue</pCommandValue>
  </Command>
  <IntSwissKnife Name="UserSetLoad_CmdValue">
    <pVariable Name="VALUE">UserSetSelectorReg</pVariable>
    <Formula>VALUE</Formula>
  </IntSwissKnife>
  <IntReg Name="UserSetLoadReg"><Address>0x200ac</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Command Name="UserSetSave" NameSpace="Standard">
    <ToolTip>Save the current settings into the selected user set</ToolTip>
    <pIsAvailable>UserSetSaveAvailable</pIsAvailable>
    <ImposedAccessMode>WO</ImposedAccessMode>
    <pValue>UserSetSaveReg</pValue>
    <pCommandValue>UserSetSave_CmdValue</pCommandValue>
  </Command>
  <IntSwissKnife Name="UserSetSave_CmdValue">
    <pVariable Name="VALUE">UserSetSelectorReg</pVariable>
    <Formula>VALUE</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="UserSetSaveAvailable">
    <pVariable Name="VAL">UserSetSelectorReg</pVariable>
    <Formula>(VAL = 0)? 0:1</Formula>
  </IntSwissKnife>
  <IntReg Name="UserSetSaveReg"><Address>0x200b4</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- profile:user-set-default -->
  <Enumeration Name="UserSetDefault" NameSpace="Standard">
    <ToolTip>User set loaded at power-up</ToolTip>
    <EnumEntry Name="Default"><Value>0</Value></EnumEntry>
    <EnumEntry Name="UserSet1"><Value>1</Value></EnumEntry>
    <EnumEntry Name="UserSet2"><Value>2</Value></EnumEntry>
    <pValue>UserSetDefaultReg</pValue>
  </Enumeration>
  <!-- /profile:user-set-default -->
  <IntReg Name="UserSetDefaultReg"><Address>0x200ec</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- Triggers, as the Basler acA1600 and Hikrobot documents shape them: the
       selector's value indexes the mode and source registers. -->
  <Enumeration Name="TriggerSelector" NameSpace="Standard">
    <ToolTip>Trigger that TriggerMode, TriggerSource and TriggerSoftware act on</ToolTip>
    <EnumEntry Name="FrameStart"><Value>0</Value></EnumEntry>
    <EnumEntry Name="AcquisitionStart"><Value>1</Value></EnumEntry>
    <pValue>TriggerSelectorReg</pValue>
    <pSelected>TriggerMode</pSelected>
    <pSelected>TriggerSource</pSelected>
    <pSelected>TriggerSoftware</pSelected>
  </Enumeration>
  <IntReg Name="TriggerSelectorReg"><Address>0x200c0</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <Enumeration Name="TriggerMode" NameSpace="Standard">
    <ToolTip>Whether the selected trigger is active</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="On"><Value>1</Value></EnumEntry>
    <pValue>TriggerModeReg</pValue>
  </Enumeration>
  <IntReg Name="TriggerModeReg">
    <Address>0x200c4</Address>
    <pIndex Offset="4">TriggerSelectorReg</pIndex>
    <Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>
  <Enumeration Name="TriggerSource" NameSpace="Standard">
    <ToolTip>Signal the selected trigger waits for</ToolTip>
    <EnumEntry Name="Line1"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Software"><Value>1</Value></EnumEntry>
    <pValue>TriggerSourceReg</pValue>
  </Enumeration>
  <IntReg Name="TriggerSourceReg">
    <Address>0x200d0</Address>
    <pIndex Offset="4">TriggerSelectorReg</pIndex>
    <Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>
  <Command Name="TriggerSoftware" NameSpace="Standard">
    <ToolTip>Fire the selected trigger once</ToolTip>
    <pValue>TriggerSoftwareReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="TriggerSoftwareReg"><Address>0x200e0</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

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
    <pValue>AcquisitionStopReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="AcquisitionStopReg"><Address>0x20028</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Boolean Name="AcquisitionFrameRateEnable" NameSpace="Standard">
    <ToolTip>Enable manual control of AcquisitionFrameRate. When false, the frame rate is unavailable for read/write.</ToolTip>
    <pValue>AcquisitionFrameRateEnableReg</pValue>
  </Boolean>
  <IntReg Name="AcquisitionFrameRateEnableReg"><Address>0x20054</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
    <pIsImplemented>FrameRateControlInq_Bit</pIsImplemented>
  </IntReg>

  <!-- profile:frame-rate -->
  <Float Name="AcquisitionFrameRate" NameSpace="Standard">
    <ToolTip>Target frame rate in Hz</ToolTip>
    <Address>0x2002c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1.0</Min>
    <Max>120.0</Max>
    <Unit>Hz</Unit>
    <Endianess>BigEndian</Endianess>
    <pIsAvailable>AcquisitionFrameRateEnable</pIsAvailable>
  </Float>
  <!-- /profile:frame-rate -->

  <!-- profile:exposure -->
  <Float Name="ExposureTime" NameSpace="Standard">
    <ToolTip>Exposure time in microseconds — locked to RO when ExposureAuto is not Off</ToolTip>
    <pValue>ExposureTimeReg</pValue>
    <Unit>us</Unit>
    <Min>10.0</Min>
    <Max>1000000.0</Max>
    <pIsImplemented>ExposureTime_Imp</pIsImplemented>
    <pIsAvailable>ExposureTime_Avl</pIsAvailable>
    <pIsLocked>ExposureAutoActive</pIsLocked>
  </Float>
  <!-- /profile:exposure -->
  <!-- The FLIR shape: the register, not the feature, carries <pInvalidator>,
       and it names the command's pValue register rather than the command. So
       executing UserSetLoad writes UserSetLoadReg, which marks this register
       stale, which marks ExposureTime stale through its <pValue> (GA-24). -->
  <FloatReg Name="ExposureTimeReg">
    <Address>0x20030</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Endianess>BigEndian</Endianess>
    <pInvalidator>UserSetLoadReg</pInvalidator>
  </FloatReg>

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

  <!-- profile:gain -->
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
  <!-- /profile:gain -->

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

  <!-- A variable the host sets while it streams, with no register behind
       it: FLIR, Basler and Hikrobot declare it so (27 corpus documents use an
       Integer). -->
  <!-- profile:tl-params-locked -->
  <Integer Name="TLParamsLocked">
    <ToolTip>Indicates whether a live grab is under way</ToolTip>
    <Visibility>Invisible</Visibility>
    <Value>0</Value>
    <Min>0</Min>
    <Max>1</Max>
  </Integer>
  <!-- /profile:tl-params-locked -->

  <Integer Name="GevTimestampTickFrequency" NameSpace="Standard">
    <ToolTip>Device timestamp tick frequency in Hz (1 GHz)</ToolTip>
    <Address>0x20060</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <!-- An Integer over an IntReg, as every timestamp in the vendor corpus is
       declared, and with nothing linking it to the latch: the cache holds the
       reading in the register node. -->
  <Integer Name="GevTimestampValue" NameSpace="Standard">
    <ToolTip>Current device timestamp in ticks (latched)</ToolTip>
    <pValue>GevTimestampValueReg</pValue>
  </Integer>
  <IntReg Name="GevTimestampValueReg">
    <Address>0x20068</Address>
    <Length>8</Length>
    <AccessMode>RO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>

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
    <pValue>TimestampLatchReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <IntReg Name="TimestampLatchReg"><Address>0x20070</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

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
    <EnumEntry Name="FrameID"><Value>4</Value></EnumEntry>
    <EnumEntry Name="ImageInfo" NameSpace="Custom"><Value>5</Value></EnumEntry>
    <pValue>ChunkSelectorReg</pValue>
  </Enumeration>
  <IntReg Name="ChunkSelectorReg"><Address>0x20084</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Boolean Name="ChunkEnable" NameSpace="Standard">
    <ToolTip>Enable the selected chunk feature</ToolTip>
    <pValue>ChunkEnableReg</pValue>
  </Boolean>
  <IntReg Name="ChunkEnableReg"><Address>0x20088</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- Chunk ports (GenICam Standard v2.1.1 §2.8.16): each register below
       addresses one chunk of a frame, from the chunk's start, not device
       memory. The shapes are the corpus's: FLIR declares its chunk values
       little-endian at offset 0 of their own chunks; Baumer's HXG20 reads
       ChunkImageLength at address -4, "offset from the back of the chunk". -->
  <Port Name="ChunkImagePort" NameSpace="Custom"><ChunkID>00000001</ChunkID></Port>
  <Port Name="ChunkTimestampPort" NameSpace="Custom"><ChunkID>00000002</ChunkID></Port>
  <Port Name="ChunkExposureTimePort" NameSpace="Custom"><ChunkID>00000003</ChunkID></Port>
  <Port Name="ChunkFrameIDPort" NameSpace="Custom"><ChunkID>05040003</ChunkID></Port>
  <Port Name="ChunkImageInfoPort" NameSpace="Custom"><ChunkID>A5A50100</ChunkID></Port>

  <Integer Name="ChunkTimestamp" NameSpace="Standard">
    <ToolTip>Timestamp of the image, in ticks</ToolTip>
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkTimestampReg</pValue>
  </Integer>
  <IntReg Name="ChunkTimestampReg"><Address>0x0</Address><Length>8</Length><AccessMode>RO</AccessMode><pPort>ChunkTimestampPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess></IntReg>

  <Float Name="ChunkExposureTime" NameSpace="Standard">
    <ToolTip>Exposure time used to capture the image</ToolTip>
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkExposureTimeReg</pValue>
    <Unit>us</Unit>
  </Float>
  <FloatReg Name="ChunkExposureTimeReg"><Address>0x0</Address><Length>8</Length><AccessMode>RO</AccessMode><pPort>ChunkExposureTimePort</pPort><Endianess>LittleEndian</Endianess></FloatReg>

  <Integer Name="ChunkFrameID" NameSpace="Standard">
    <ToolTip>Frame id of the image</ToolTip>
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkFrameIDReg</pValue>
  </Integer>
  <IntReg Name="ChunkFrameIDReg"><Address>0x0</Address><Length>8</Length><AccessMode>RO</AccessMode><pPort>ChunkFrameIDPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess></IntReg>

  <Integer Name="ChunkWidth" NameSpace="Standard">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkWidthReg</pValue>
  </Integer>
  <IntReg Name="ChunkWidthReg"><Address>0x0</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>ChunkImageInfoPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="ChunkHeight" NameSpace="Standard">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkHeightReg</pValue>
  </Integer>
  <IntReg Name="ChunkHeightReg"><Address>0x4</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>ChunkImageInfoPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="ChunkImageLength" NameSpace="Custom">
    <ToolTip>Bytes of image data, read from the end of the ImageInfo chunk</ToolTip>
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>ChunkImageLengthReg</pValue>
  </Integer>
  <IntReg Name="ChunkImageLengthReg"><Address>-4</Address><Length>4</Length><AccessMode>RO</AccessMode><pPort>ChunkImageInfoPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- Event delivery is selected through GenApi, not through a bootstrap
       register: EventSelector picks a GigE Vision event id and
       EventNotification turns it on. The ids are the standard ones
       (GEV_EVENT_START_OF_TRANSFER = 0x0005, END_OF_TRANSFER = 0x0006). -->
  <Enumeration Name="EventSelector" NameSpace="Standard">
    <ToolTip>Select which event to configure</ToolTip>
    <EnumEntry Name="StartOfTransfer"><Value>5</Value></EnumEntry>
    <EnumEntry Name="EndOfTransfer"><Value>6</Value></EnumEntry>
    <EnumEntry Name="ExposureEnd"><Value>0x9001</Value></EnumEntry>
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

  <!-- The ExposureEnd event port: its registers address the event as it was
       sent, header included — the event id at 0x2 and the timestamp at 0x8,
       where every corpus document puts them, and the frame id in the event
       data at 0x10, little-endian, as FLIR declares EventExposureEndFrameID. -->
  <Port Name="EventExposureEndPort"><EventID>9001</EventID></Port>
  <Integer Name="EventExposureEnd" NameSpace="Standard">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>EventExposureEndReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndReg"><Address>0x2</Address><Length>2</Length><AccessMode>RO</AccessMode><pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <Integer Name="EventExposureEndTimestamp" NameSpace="Standard">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>EventExposureEndTimestampReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndTimestampReg"><Address>0x8</Address><Length>8</Length><AccessMode>RO</AccessMode><pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <Integer Name="EventExposureEndFrameID" NameSpace="Standard">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>EventExposureEndFrameIDReg</pValue>
  </Integer>
  <IntReg Name="EventExposureEndFrameIDReg"><Address>0x10</Address><Length>8</Length><AccessMode>RO</AccessMode><pPort>EventExposureEndPort</pPort><Sign>Unsigned</Sign><Endianess>LittleEndian</Endianess></IntReg>

</RegisterDescription>
"#;

/// Which GenApi XML the fake serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum XmlProfile {
    /// [`FAKE_XML`]: SFNC 2 names — `ExposureTime`, `Gain`,
    /// `AcquisitionFrameRate`, `UserSetDefault` — with `TLParamsLocked` an
    /// `<Integer>`.
    #[default]
    Sfnc,
    /// The names older cameras use instead, as the Basler acA1600-20gm
    /// declares them (`Basler_acA1600_20gm.xml` in the vendor corpus):
    /// `ExposureTimeAbs` with no `ExposureTime`, `GainRaw` (an `<Integer>`
    /// per `GainSelector` entry) with no `Gain`, `AcquisitionFrameRateAbs`,
    /// `UserSetDefaultSelector`; and `TLParamsLocked` a `<Boolean>` with a
    /// `<Value>`, as AVT and Prosilica declare it. Same registers, same
    /// behaviour.
    Legacy,
}

/// The legacy profile's replacement for each `<!-- profile:NAME -->` section
/// of [`FAKE_XML`].
const LEGACY_SECTIONS: &[(&str, &str)] = &[
    ("exposure-category", "<pFeature>ExposureTimeAbs</pFeature>"),
    (
        "exposure",
        r#"<Float Name="ExposureTimeAbs" NameSpace="Standard">
    <ToolTip>Directly sets the camera's exposure time in microseconds</ToolTip>
    <pValue>ExposureTimeReg</pValue>
    <Min>10.0</Min>
    <Max>1000000.0</Max>
    <Unit>us</Unit>
    <pIsLocked>ExposureAutoActive</pIsLocked>
  </Float>"#,
    ),
    (
        "frame-rate-category",
        "<pFeature>AcquisitionFrameRateAbs</pFeature>",
    ),
    (
        "frame-rate",
        r#"<Float Name="AcquisitionFrameRateAbs" NameSpace="Standard">
    <ToolTip>Acquisition frame rate in frames per second, when enabled</ToolTip>
    <Address>0x2002c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1.0</Min>
    <Max>120.0</Max>
    <Unit>Hz</Unit>
    <Endianess>BigEndian</Endianess>
    <pIsAvailable>AcquisitionFrameRateEnable</pIsAvailable>
  </Float>"#,
    ),
    (
        "gain-category",
        "<pFeature>GainSelector</pFeature><pFeature>GainRaw</pFeature>",
    ),
    (
        "gain",
        r#"<Enumeration Name="GainSelector" NameSpace="Standard">
    <ToolTip>Selects the gain control to configure</ToolTip>
    <EnumEntry Name="All"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Tap1"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Tap2"><Value>2</Value></EnumEntry>
    <pValue>GainSelectorReg</pValue>
    <pSelected>GainRaw</pSelected>
  </Enumeration>
  <IntReg Name="GainSelectorReg"><Address>0x200fc</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
  <Integer Name="GainRaw" NameSpace="Standard">
    <ToolTip>Sets the selected gain control in device specific units</ToolTip>
    <pValue>GainRawReg</pValue>
    <Min>0</Min>
    <Max>1023</Max>
    <pIsLocked>GainAutoActive</pIsLocked>
  </Integer>
  <IntReg Name="GainRawReg">
    <Address>0x200f0</Address>
    <pIndex Offset="4">GainSelectorReg</pIndex>
    <Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
  </IntReg>"#,
    ),
    (
        "user-set-default-category",
        "<pFeature>UserSetDefaultSelector</pFeature>",
    ),
    (
        "user-set-default",
        r#"<Enumeration Name="UserSetDefaultSelector" NameSpace="Standard">
    <ToolTip>Selects the user set loaded at power-up</ToolTip>
    <EnumEntry Name="Default"><Value>0</Value></EnumEntry>
    <EnumEntry Name="UserSet1"><Value>1</Value></EnumEntry>
    <EnumEntry Name="UserSet2"><Value>2</Value></EnumEntry>
    <pValue>UserSetDefaultReg</pValue>
  </Enumeration>"#,
    ),
    (
        "tl-params-locked",
        r#"<Boolean Name="TLParamsLocked" NameSpace="Standard">
    <Value>0</Value>
    <OnValue>1</OnValue>
    <OffValue>0</OffValue>
  </Boolean>"#,
    ),
];

/// `GainSelector`'s register in the legacy profile.
pub const REG_GAIN_SELECTOR: u64 = 0x200fc;

/// The GenApi XML a camera with this profile serves.
pub fn genapi_xml(profile: XmlProfile) -> std::borrow::Cow<'static, str> {
    match profile {
        XmlProfile::Sfnc => std::borrow::Cow::Borrowed(FAKE_XML),
        XmlProfile::Legacy => {
            let mut xml = FAKE_XML.to_string();
            for (section, replacement) in LEGACY_SECTIONS {
                let begin = format!("<!-- profile:{section} -->");
                let end = format!("<!-- /profile:{section} -->");
                let start = xml
                    .find(&begin)
                    .unwrap_or_else(|| panic!("FAKE_XML has no {begin}"));
                let stop = xml[start..]
                    .find(&end)
                    .map(|at| start + at + end.len())
                    .unwrap_or_else(|| panic!("FAKE_XML has no {end}"));
                xml.replace_range(start..stop, replacement);
            }
            std::borrow::Cow::Owned(xml)
        }
    }
}

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
    /// `ChunkSelector` values whose `ChunkEnable` is on: a selected feature,
    /// as `EventNotification` is.
    enabled_chunks: HashSet<u32>,
    /// When the controller last sent a register-access command, for the
    /// heartbeat rule.
    last_register_command: Instant,
    /// The application holding control privilege: the address and port its
    /// commands come from. `None` while nobody holds it.
    controller: Option<SocketAddr>,
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
    /// User sets saved by `UserSetSave`, by set number: the registers a
    /// `UserSetLoad` of that set restores.
    user_sets: HashMap<u32, Vec<(u64, Vec<u8>)>>,
    /// `TriggerSoftware` executions for `FrameStart` not yet turned into a
    /// frame.
    pending_frame_triggers: u32,
    /// Every `TriggerSoftware` execution.
    software_triggers: u64,
}

/// Registers a user set holds: the analog-control settings.
const USER_SET_REGISTERS: &[u64] = &[
    REG_EXPOSURE_TIME,
    REG_GAIN,
    REG_EXPOSURE_AUTO,
    REG_GAIN_AUTO,
];

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
    pub fn new(
        width: u32,
        height: u32,
        pixel_format: u32,
        zip_xml: bool,
        profile: XmlProfile,
    ) -> Self {
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
        regs.insert(
            GVCP_CAPABILITY,
            (CAPABILITY_CCP_APPLICATION_PORT_IP
                | CAPABILITY_PENDING_ACK
                | CAPABILITY_PACKET_RESEND
                | CAPABILITY_WRITEMEM
                | CAPABILITY_CONCATENATION)
                .to_be_bytes()
                .to_vec(),
        );
        regs.insert(GVCP_CONFIGURATION, 0u32.to_be_bytes().to_vec());
        regs.insert(HEARTBEAT_TIMEOUT, 3000u32.to_be_bytes().to_vec());
        regs.insert(
            TIMESTAMP_TICK_FREQUENCY_HIGH,
            ((TICK_FREQUENCY_HZ >> 32) as u32).to_be_bytes().to_vec(),
        );
        regs.insert(
            TIMESTAMP_TICK_FREQUENCY_LOW,
            (TICK_FREQUENCY_HZ as u32).to_be_bytes().to_vec(),
        );
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
        regs.insert(REG_USER_SET_SAVE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_SOFTWARE_SIGNAL_PULSE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_USER_SET_DEFAULT, 0u32.to_be_bytes().to_vec());
        // Triggers off, waiting on Line1, for both selector entries.
        regs.insert(
            REG_TRIGGER_SELECTOR,
            TRIGGER_FRAME_START.to_be_bytes().to_vec(),
        );
        for index in 0..2u64 {
            regs.insert(REG_TRIGGER_MODE + 4 * index, 0u32.to_be_bytes().to_vec());
            regs.insert(
                REG_TRIGGER_SOURCE + 4 * index,
                TRIGGER_SOURCE_LINE1.to_be_bytes().to_vec(),
            );
        }
        regs.insert(REG_TRIGGER_SOFTWARE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_BINNING_HORIZONTAL, 1u32.to_be_bytes().to_vec());
        regs.insert(REG_BINNING_VERTICAL, 1u32.to_be_bytes().to_vec());
        regs.insert(REG_GAIN_SELECTOR, 0u32.to_be_bytes().to_vec());
        for index in 0..3u64 {
            regs.insert(REG_GAIN_RAW + 4 * index, 0u32.to_be_bytes().to_vec());
        }
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
        let xml = genapi_xml(profile);
        let (xml_blob, xml_name) = if zip_xml {
            (zip_xml_blob(xml.as_bytes()), "fake.zip")
        } else {
            (xml.as_bytes().to_vec(), "fake.xml")
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
            enabled_chunks: HashSet::new(),
            last_register_command: Instant::now(),
            controller: None,
            enforce_heartbeat: false,
            max_packet_size: None,
            max_on_wire: None,
            pending_test_packet: None,
            user_sets: HashMap::new(),
            pending_frame_triggers: 0,
            software_triggers: 0,
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

    /// Apply the heartbeat rule to a register-access command from `peer`, and
    /// report whether privilege was just revoked.
    ///
    /// Called for register-access commands only. Discovery and FORCEIP are
    /// broadcast by any application on the subnet, so counting them would let an
    /// unrelated `viva-camctl list` hold another application's privilege open.
    /// Only the controller's own commands refresh its timer: a monitor reading
    /// registers keeps nobody's privilege alive.
    pub fn note_register_command(&mut self, peer: SocketAddr) -> bool {
        let revoked = self.enforce_heartbeat
            && self.controller.is_some()
            && self.heartbeat_expired(self.last_register_command.elapsed());
        if revoked {
            self.revoke_control();
        }
        if self.controller == Some(peer) {
            self.last_register_command = Instant::now();
        }
        revoked
    }

    fn heartbeat_expired(&self, elapsed: Duration) -> bool {
        let timeout = Duration::from_millis(u64::from(self.heartbeat_timeout_ms()));
        !timeout.is_zero() && elapsed > timeout
    }

    /// Give control privilege up on the device side, as a device does when its
    /// heartbeat expires: CCP reads 0 and nobody is the controller.
    pub fn revoke_control(&mut self) {
        self.controller = None;
        self.regs.insert(CCP, 0u32.to_be_bytes().to_vec());
    }

    /// The application holding control privilege, if any.
    pub fn controller(&self) -> Option<SocketAddr> {
        self.controller
    }

    /// Whether `peer` may access the device's registers.
    ///
    /// While an application holds control, another may still read (it
    /// monitors) unless the privilege is exclusive, and may not write. While
    /// nobody holds control every application may do both, as aravis's fake
    /// camera allows (`arvgvfakecamera.c`); what a real device does then is
    /// not something we have seen.
    pub fn may_access(&self, peer: SocketAddr, write: bool) -> bool {
        match self.controller {
            None => true,
            Some(controller) if controller == peer => true,
            Some(_) if write => false,
            Some(_) => self.read_u32_be(CCP) & CCP_EXCLUSIVE_ACCESS == 0,
        }
    }

    /// A write of `value` to CCP from `peer`, and whether it was accepted.
    /// Claims control with either access bit set, gives it up with neither.
    /// Refused while another application holds control: this device offers
    /// no switchover.
    pub fn write_ccp(&mut self, peer: SocketAddr, value: u32) -> bool {
        if self.controller.is_some_and(|controller| controller != peer) {
            return false;
        }
        if value & (CCP_CONTROL_ACCESS | CCP_EXCLUSIVE_ACCESS) != 0 {
            self.controller = Some(peer);
            self.last_register_command = Instant::now();
            self.regs.insert(CCP, value.to_be_bytes().to_vec());
        } else {
            self.revoke_control();
        }
        true
    }

    /// Whether this device answers a slow command with `PENDING_ACK`: it
    /// says it can (GVCP Capability) and the controller has enabled it (GVCP
    /// Configuration).
    pub fn pending_ack_enabled(&self) -> bool {
        self.read_u32_be(GVCP_CAPABILITY) & CAPABILITY_PENDING_ACK != 0
            && self.read_u32_be(GVCP_CONFIGURATION) & CONFIGURATION_PENDING_ACK_ENABLE != 0
    }

    /// Set or clear the `PENDING_ACK` bit of the capability register.
    pub fn set_pending_ack_supported(&mut self, supported: bool) {
        let mut caps = self.read_u32_be(GVCP_CAPABILITY);
        if supported {
            caps |= CAPABILITY_PENDING_ACK;
        } else {
            caps &= !CAPABILITY_PENDING_ACK;
        }
        self.regs
            .insert(GVCP_CAPABILITY, caps.to_be_bytes().to_vec());
    }

    /// Report `ip`, `mask` and `gateway` as the current IP configuration
    /// (bootstrap `0x0024`, `0x0034`, `0x0044`), as the Discovery ACK does.
    pub fn set_network(&mut self, ip: Ipv4Addr, mask: Ipv4Addr, gateway: Ipv4Addr) {
        self.regs.insert(CURRENT_IP_ADDRESS, ip.octets().to_vec());
        self.regs
            .insert(CURRENT_SUBNET_MASK, mask.octets().to_vec());
        self.regs
            .insert(CURRENT_DEFAULT_GATEWAY, gateway.octets().to_vec());
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

    /// Report `hz` as the timestamp tick frequency, the unit of `GevSCPD`.
    /// `0` is what a device without a timestamp clock reports.
    pub fn set_tick_frequency(&mut self, hz: u64) {
        self.regs.insert(
            TIMESTAMP_TICK_FREQUENCY_HIGH,
            ((hz >> 32) as u32).to_be_bytes().to_vec(),
        );
        self.regs.insert(
            TIMESTAMP_TICK_FREQUENCY_LOW,
            (hz as u32).to_be_bytes().to_vec(),
        );
    }

    /// Set or clear the `PACKETRESEND_CMD` bit of the capability register.
    pub fn set_packet_resend(&mut self, supported: bool) {
        let mut caps = self.read_u32_be(GVCP_CAPABILITY);
        if supported {
            caps |= CAPABILITY_PACKET_RESEND;
        } else {
            caps &= !CAPABILITY_PACKET_RESEND;
        }
        self.regs
            .insert(GVCP_CAPABILITY, caps.to_be_bytes().to_vec());
    }

    /// Whether the capability register says this device answers
    /// `PACKETRESEND_CMD`.
    pub fn packet_resend_supported(&self) -> bool {
        self.read_u32_be(GVCP_CAPABILITY) & CAPABILITY_PACKET_RESEND != 0
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

        // The primary application is whoever holds control right now.
        if addr == PRIMARY_APPLICATION_PORT || addr == PRIMARY_APPLICATION_IP {
            let word = match (addr, self.controller) {
                (PRIMARY_APPLICATION_PORT, Some(owner)) => u32::from(owner.port()),
                (PRIMARY_APPLICATION_IP, Some(SocketAddr::V4(owner))) => u32::from(*owner.ip()),
                _ => 0,
            };
            let mut result = word.to_be_bytes().to_vec();
            result.resize(len, 0);
            return result;
        }

        // `EventNotification` reports the selected event, not a stored word.
        if addr == REG_EVENT_NOTIFICATION {
            let on = u32::from(self.event_notification_on(self.event_selector()));
            let mut result = on.to_be_bytes().to_vec();
            result.resize(len, 0);
            result.truncate(len);
            return result;
        }

        if addr == REG_CHUNK_ENABLE {
            let on = u32::from(self.chunk_enabled(self.chunk_selector()));
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

        if addr == REG_CHUNK_ENABLE && data.len() >= 4 {
            let selector = self.chunk_selector();
            if u32::from_be_bytes([data[0], data[1], data[2], data[3]]) != 0 {
                self.enabled_chunks.insert(selector);
            } else {
                self.enabled_chunks.remove(&selector);
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
            let set = self.read_u32_be(REG_USER_SET_LOAD);
            self.load_user_set(set);
        }
        if addr == REG_USER_SET_SAVE {
            let set = self.read_u32_be(REG_USER_SET_SAVE);
            self.save_user_set(set);
        }
        if addr == REG_TRIGGER_SOFTWARE {
            self.software_triggers += 1;
            if self.read_u32_be(REG_TRIGGER_SELECTOR) == TRIGGER_FRAME_START {
                self.pending_frame_triggers += 1;
            }
        }
    }

    /// Whether the next frame may start now. Always, while the `FrameStart`
    /// trigger is `Off`; once per `TriggerSoftware` while it is `On` with
    /// source `Software`; never while it waits on a line, which nothing
    /// drives here.
    pub fn take_frame_start(&mut self) -> bool {
        let index = u64::from(TRIGGER_FRAME_START);
        if self.read_u32_be(REG_TRIGGER_MODE + 4 * index) == 0 {
            return true;
        }
        if self.read_u32_be(REG_TRIGGER_SOURCE + 4 * index) != TRIGGER_SOURCE_SOFTWARE
            || self.pending_frame_triggers == 0
        {
            return false;
        }
        self.pending_frame_triggers -= 1;
        true
    }

    /// How many times `TriggerSoftware` was executed.
    pub fn software_triggers(&self) -> u64 {
        self.software_triggers
    }

    fn read_u32_be(&self, addr: u64) -> u32 {
        let data = self.read(addr, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Restore user set `set`, as `UserSetLoad` does on a real camera: `0` is
    /// `Default`, the analog-control defaults the fake boots with, and so is a
    /// set never saved.
    ///
    /// A command that only acknowledges the write is untestable: a test could
    /// assert nothing beyond the absence of an error, which is the fake
    /// agreeing with itself. Restoring observable device state means a test can
    /// change a feature, execute the command, and read the change back out
    /// (ADR-0019).
    fn load_user_set(&mut self, set: u32) {
        if let Some(saved) = self.user_sets.get(&set).cloned() {
            for (addr, bytes) in saved {
                self.regs.insert(addr, bytes);
            }
            return;
        }
        self.regs.insert(
            REG_EXPOSURE_TIME,
            DEFAULT_EXPOSURE_US.to_be_bytes().to_vec(),
        );
        self.regs.insert(REG_GAIN, 0.0f64.to_be_bytes().to_vec());
        self.regs
            .insert(REG_EXPOSURE_AUTO, 0u32.to_be_bytes().to_vec());
        self.regs.insert(REG_GAIN_AUTO, 0u32.to_be_bytes().to_vec());
    }

    /// Save the current settings into user set `set`. `Default` (`0`) is the
    /// factory set and cannot be overwritten, so a write of `0` is ignored.
    fn save_user_set(&mut self, set: u32) {
        if set == 0 {
            return;
        }
        let saved = USER_SET_REGISTERS
            .iter()
            .filter_map(|addr| self.regs.get(addr).map(|bytes| (*addr, bytes.clone())))
            .collect();
        self.user_sets.insert(set, saved);
    }

    /// The exposure saved in user set `set`, or `None` if nothing has been
    /// saved there.
    ///
    /// Device state, read past the protocol: a test can check *which* set a
    /// `UserSetSave` landed in without trusting the client's own account of
    /// what it wrote.
    pub fn saved_exposure(&self, set: u32) -> Option<f64> {
        let saved = self.user_sets.get(&set)?;
        let (_, bytes) = saved.iter().find(|(addr, _)| *addr == REG_EXPOSURE_TIME)?;
        Some(f64::from_be_bytes(bytes.as_slice().try_into().ok()?))
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

    /// `GevSCPD`: the delay between stream packets, in timestamp ticks.
    pub fn stream_packet_delay_ticks(&self) -> u32 {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_PACKET_DELAY, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Timestamp tick frequency from bootstrap 0x093C (high) and 0x0940 (low).
    pub fn tick_frequency(&self) -> u64 {
        let high = self.read(TIMESTAMP_TICK_FREQUENCY_HIGH, 4);
        let low = self.read(TIMESTAMP_TICK_FREQUENCY_LOW, 4);
        (u64::from(u32::from_be_bytes([high[0], high[1], high[2], high[3]])) << 32)
            | u64::from(u32::from_be_bytes([low[0], low[1], low[2], low[3]]))
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

    /// Value currently selected by `ChunkSelector`.
    pub fn chunk_selector(&self) -> u32 {
        let data = self.read(REG_CHUNK_SELECTOR, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Whether `ChunkEnable` is on for `selector`, whichever chunk the
    /// controller selected last.
    pub fn chunk_enabled(&self, selector: u32) -> bool {
        self.enabled_chunks.contains(&selector)
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
        RegisterMap::new(64, 48, 0x0108_0001, false, XmlProfile::Sfnc)
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

    /// `ChunkEnable` is a selected feature too: enabling two chunks writes
    /// the same address twice, with `ChunkSelector` changed in between.
    #[test]
    fn chunk_enable_follows_the_selector() {
        let mut regs = map();
        for selector in [CHUNK_SELECTOR_TIMESTAMP, CHUNK_SELECTOR_FRAME_ID] {
            regs.write(REG_CHUNK_SELECTOR, &selector.to_be_bytes());
            regs.write(REG_CHUNK_ENABLE, &1u32.to_be_bytes());
        }
        assert!(regs.chunk_enabled(CHUNK_SELECTOR_TIMESTAMP));
        assert!(regs.chunk_enabled(CHUNK_SELECTOR_FRAME_ID));
        assert!(!regs.chunk_enabled(CHUNK_SELECTOR_IMAGE_INFO));
        regs.write(REG_CHUNK_SELECTOR, &CHUNK_SELECTOR_IMAGE_INFO.to_be_bytes());
        assert_eq!(regs.read(REG_CHUNK_ENABLE, 4), vec![0, 0, 0, 0]);
        regs.write(REG_CHUNK_SELECTOR, &CHUNK_SELECTOR_FRAME_ID.to_be_bytes());
        assert_eq!(regs.read(REG_CHUNK_ENABLE, 4), vec![0, 0, 0, 1]);
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

    /// GigE Vision bootstrap: tick frequency high at 0x093C, low at 0x0940,
    /// so `GevSCPD` (in ticks) can be converted to time.
    #[test]
    fn the_tick_frequency_is_published_at_bootstrap() {
        let regs = map();
        assert_eq!(regs.read(0x093C, 4), vec![0, 0, 0, 0]);
        assert_eq!(
            regs.read(0x0940, 4),
            1_000_000_000u32.to_be_bytes().to_vec()
        );
        assert_eq!(regs.tick_frequency(), 1_000_000_000);
    }

    /// GVCP Capability at 0x0934: the primary application's IP and port
    /// (mask 0x0800_0000), `PENDING_ACK` (0x20) and packet resend (0x4) on
    /// by default, resend switchable.
    #[test]
    fn the_capability_register_advertises_packet_resend() {
        let mut regs = map();
        assert_eq!(regs.read(0x0934, 4), vec![0x08, 0, 0, 0x27]);
        assert!(regs.packet_resend_supported());
        regs.set_packet_resend(false);
        assert_eq!(regs.read(0x0934, 4), vec![0x08, 0, 0, 0x23]);
        assert!(!regs.packet_resend_supported());
    }

    /// `PENDING_ACK` is sent only when the capability says so *and* the
    /// controller set bit `0x2` of GVCP Configuration (0x0954).
    #[test]
    fn pending_ack_needs_the_capability_and_the_enable_bit() {
        let mut regs = map();
        assert!(!regs.pending_ack_enabled(), "off until enabled");
        regs.write(0x0954, &2u32.to_be_bytes());
        assert!(regs.pending_ack_enabled());
        regs.set_pending_ack_supported(false);
        assert!(!regs.pending_ack_enabled(), "not without the capability");
    }

    /// CCP belongs to the address and port that claimed it: another may read
    /// but not write or claim, until the owner writes 0. With the exclusive
    /// bit, another may not even read.
    #[test]
    fn control_belongs_to_the_application_that_claimed_it() {
        let owner: SocketAddr = "127.0.0.1:50001".parse().unwrap();
        let other: SocketAddr = "127.0.0.1:50002".parse().unwrap();
        let mut regs = map();
        assert!(regs.may_access(other, true), "nobody controls yet");
        assert!(regs.write_ccp(owner, CCP_CONTROL_ACCESS));
        assert_eq!(regs.controller(), Some(owner));
        assert_eq!(
            regs.read(PRIMARY_APPLICATION_PORT, 4),
            50001u32.to_be_bytes()
        );
        assert_eq!(regs.read(PRIMARY_APPLICATION_IP, 4), [127, 0, 0, 1]);
        assert!(regs.may_access(other, false));
        assert!(!regs.may_access(other, true));
        assert!(!regs.write_ccp(other, CCP_CONTROL_ACCESS));
        assert!(!regs.write_ccp(other, 0), "only the owner releases");
        assert!(regs.write_ccp(owner, CCP_CONTROL_ACCESS | CCP_EXCLUSIVE_ACCESS));
        assert!(!regs.may_access(other, false));
        assert!(regs.write_ccp(owner, 0));
        assert_eq!(regs.controller(), None);
        assert_eq!(regs.read(PRIMARY_APPLICATION_IP, 4), [0, 0, 0, 0]);
        assert_eq!(regs.read(CCP, 4), vec![0, 0, 0, 0]);
    }

    #[test]
    fn the_tick_frequency_can_be_changed() {
        let mut regs = map();
        regs.set_tick_frequency(125_000_000);
        assert_eq!(regs.tick_frequency(), 125_000_000);
        assert_eq!(regs.read(0x0940, 4), 125_000_000u32.to_be_bytes().to_vec());
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

    /// The legacy profile replaces every marked section, so it declares the
    /// older names and none of the SFNC 2 ones it stands in for.
    #[test]
    fn the_legacy_profile_replaces_every_section() {
        let xml = genapi_xml(XmlProfile::Legacy);
        assert!(!xml.contains("<!-- profile:"), "a section was left in");
        for present in [
            r#"Name="ExposureTimeAbs""#,
            r#"Name="GainRaw""#,
            r#"Name="GainSelector""#,
            r#"Name="AcquisitionFrameRateAbs""#,
            r#"Name="UserSetDefaultSelector""#,
            r#"<Boolean Name="TLParamsLocked""#,
        ] {
            assert!(xml.contains(present), "missing {present}");
        }
        for absent in [
            r#"<Float Name="ExposureTime""#,
            r#"<Float Name="Gain""#,
            r#"<Float Name="AcquisitionFrameRate""#,
            r#"<Enumeration Name="UserSetDefault""#,
            "<pFeature>Gain</pFeature>",
        ] {
            assert!(!xml.contains(absent), "still declares {absent}");
        }
    }
}
