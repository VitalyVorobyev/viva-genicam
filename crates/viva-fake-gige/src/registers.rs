//! In-memory bootstrap register map and embedded GenApi XML.
//!
//! # Register Address Map
//!
//! | Address     | Length | Feature                         | Type     |
//! |-------------|--------|---------------------------------|----------|
//! | `0x0a00`    | 4      | CCP (Control Channel Privilege) | u32 BE   |
//! | `0x0938`    | 4      | Heartbeat Timeout               | u32 BE   |
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
//! | `0x20060`   | 4      | GevTimestampTickFrequency (RO)  | u32 BE   |
//! | `0x20068`   | 8      | GevTimestampValue (RO)          | u64 BE   |
//! | `0x20070`   | 4      | TimestampLatch (command)        | u32 BE   |
//! | `0x20080`   | 4      | ChunkModeActive                 | u32 BE   |
//! | `0x20084`   | 4      | ChunkSelector                   | u32 BE   |
//! | `0x20088`   | 4      | ChunkEnable                     | u32 BE   |
//! | `0x20100`   | 4      | WidthMin (RO)                   | u32 BE   |
//! | `0x20104`   | 4      | WidthMax (RO)                   | u32 BE   |
//! | `0x20108`   | 4      | HeightMin (RO)                  | u32 BE   |
//! | `0x2010c`   | 4      | HeightMax (RO)                  | u32 BE   |
//! | `0x20200`   | 32     | DeviceModelName (RO)            | string   |
//! | `0x20220`   | 32     | DeviceVendorName (RO)           | string   |
//! | `0x20240`   | 16     | DeviceSerialNumber (RO)         | string   |
//! | `0x20260`   | 32     | DeviceFirmwareVersion (RO)      | string   |
//! | `0x20280`   | 32     | DeviceID (RO)                   | string   |

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Instant;

/// Bootstrap register addresses (GigE Vision specification).
pub const CCP: u64 = 0x0a00;
pub const HEARTBEAT_TIMEOUT: u64 = 0x0938;
pub const STREAM_CHANNEL_BASE: u64 = 0x0d00;
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

/// Timestamp registers.
pub const REG_TIMESTAMP_FREQ: u64 = 0x20060;
pub const REG_TIMESTAMP_VALUE: u64 = 0x20068;
pub const REG_TIMESTAMP_LATCH: u64 = 0x20070;

/// Chunk data registers.
pub const REG_CHUNK_MODE_ACTIVE: u64 = 0x20080;
pub const REG_CHUNK_SELECTOR: u64 = 0x20084;
pub const REG_CHUNK_ENABLE: u64 = 0x20088;

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
  </Category>

  <Category Name="ChunkDataControl">
    <DisplayName>Chunk Data Control</DisplayName>
    <pFeature>ChunkModeActive</pFeature>
    <pFeature>ChunkSelector</pFeature>
    <pFeature>ChunkEnable</pFeature>
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

  <Enumeration Name="PixelFormat" NameSpace="Standard">
    <ToolTip>Format of the pixel data</ToolTip>
    <EnumEntry Name="Mono8" NameSpace="Standard"><Value>0x01080001</Value></EnumEntry>
    <EnumEntry Name="Mono16" NameSpace="Standard"><Value>0x01100007</Value></EnumEntry>
    <EnumEntry Name="RGB8" NameSpace="Standard"><Value>0x02180014</Value></EnumEntry>
    <EnumEntry Name="BayerRG8" NameSpace="Standard"><Value>0x01080009</Value></EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>
  <IntReg Name="PixelFormatReg"><Address>0x20008</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

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

  <Command Name="AcquisitionStop" NameSpace="Standard">
    <ToolTip>Stop image acquisition</ToolTip>
    <Address>0x20028</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

  <Float Name="AcquisitionFrameRate" NameSpace="Standard">
    <ToolTip>Target frame rate in Hz</ToolTip>
    <Address>0x2002c</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>1.0</Min>
    <Max>120.0</Max>
    <Endianess>BigEndian</Endianess>
  </Float>

  <Float Name="ExposureTime" NameSpace="Standard">
    <ToolTip>Exposure time in microseconds</ToolTip>
    <Address>0x20030</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>10.0</Min>
    <Max>1000000.0</Max>
    <Endianess>BigEndian</Endianess>
  </Float>

  <Enumeration Name="ExposureAuto" NameSpace="Standard">
    <ToolTip>Automatic exposure control</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Once"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Continuous"><Value>2</Value></EnumEntry>
    <pValue>ExposureAutoReg</pValue>
  </Enumeration>
  <IntReg Name="ExposureAutoReg"><Address>0x20038</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <!-- ════════════════════════════════════════════════════════════════════
       Analog Control Features
       ════════════════════════════════════════════════════════════════════ -->

  <Float Name="Gain" NameSpace="Standard">
    <ToolTip>Gain applied to the image in dB</ToolTip>
    <Address>0x20040</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>0.0</Min>
    <Max>48.0</Max>
    <Endianess>BigEndian</Endianess>
  </Float>

  <Enumeration Name="GainAuto" NameSpace="Standard">
    <ToolTip>Automatic gain control</ToolTip>
    <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Once"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Continuous"><Value>2</Value></EnumEntry>
    <pValue>GainAutoReg</pValue>
  </Enumeration>
  <IntReg Name="GainAutoReg"><Address>0x20048</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

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

  <Integer Name="ChunkModeActive" NameSpace="Standard">
    <ToolTip>Enable chunk data in image frames</ToolTip>
    <Address>0x20080</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <Max>1</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

  <Enumeration Name="ChunkSelector" NameSpace="Standard">
    <ToolTip>Select which chunk feature to configure</ToolTip>
    <EnumEntry Name="Timestamp"><Value>1</Value></EnumEntry>
    <EnumEntry Name="ExposureTime"><Value>2</Value></EnumEntry>
    <EnumEntry Name="Gain"><Value>3</Value></EnumEntry>
    <pValue>ChunkSelectorReg</pValue>
  </Enumeration>
  <IntReg Name="ChunkSelectorReg"><Address>0x20084</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Integer Name="ChunkEnable" NameSpace="Standard">
    <ToolTip>Enable the selected chunk feature</ToolTip>
    <Address>0x20088</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Min>0</Min>
    <Max>1</Max>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </Integer>

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
}

impl RegisterMap {
    /// Create a new register map with the given image dimensions.
    ///
    /// Initializes all bootstrap, feature, and device info registers with
    /// sensible defaults. The GenApi XML is embedded at [`XML_BLOB_BASE`].
    pub fn new(width: u32, height: u32) -> Self {
        let mut regs = HashMap::new();

        // ── Bootstrap registers ─────────────────────────────────────────
        regs.insert(CCP, vec![0, 0, 0, 0]);
        regs.insert(HEARTBEAT_TIMEOUT, 3000u32.to_be_bytes().to_vec());

        // Stream channel 0
        let base = STREAM_CHANNEL_BASE;
        regs.insert(base + SCP_HOST_PORT, vec![0, 0, 0, 0]);
        regs.insert(base + SCP_PACKET_SIZE, 1500u32.to_be_bytes().to_vec());
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
        regs.insert(REG_PIXEL_FORMAT, 0x01080001u32.to_be_bytes().to_vec()); // Mono8
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
        regs.insert(REG_ACQ_STOP, vec![0, 0, 0, 0]);
        regs.insert(REG_ACQ_FRAME_RATE, 30.0f32.to_be_bytes().to_vec());
        regs.insert(REG_EXPOSURE_TIME, 5000.0f64.to_be_bytes().to_vec());
        regs.insert(REG_EXPOSURE_AUTO, 0u32.to_be_bytes().to_vec()); // Off

        // ── Analog control ──────────────────────────────────────────────
        regs.insert(REG_GAIN, 0.0f64.to_be_bytes().to_vec());
        regs.insert(REG_GAIN_AUTO, 0u32.to_be_bytes().to_vec()); // Off
        regs.insert(REG_BLACK_LEVEL, 0u32.to_be_bytes().to_vec());

        // ── Timestamp (1 GHz tick frequency) ────────────────────────────
        regs.insert(REG_TIMESTAMP_FREQ, 1_000_000_000u32.to_be_bytes().to_vec());
        regs.insert(REG_TIMESTAMP_VALUE, vec![0u8; 8]);
        regs.insert(REG_TIMESTAMP_LATCH, vec![0, 0, 0, 0]);

        // ── Chunk data ──────────────────────────────────────────────────
        regs.insert(REG_CHUNK_MODE_ACTIVE, 0u32.to_be_bytes().to_vec());
        regs.insert(REG_CHUNK_SELECTOR, 1u32.to_be_bytes().to_vec()); // Timestamp
        regs.insert(REG_CHUNK_ENABLE, 0u32.to_be_bytes().to_vec());

        // ── XML URL register ────────────────────────────────────────────
        let xml_blob = FAKE_XML.as_bytes().to_vec();
        let url = format!("Local:fake.xml;{:x};{:x}\0", XML_BLOB_BASE, xml_blob.len());
        let mut url_bytes = vec![0u8; URL_REG_LEN];
        let src = url.as_bytes();
        url_bytes[..src.len()].copy_from_slice(src);
        regs.insert(FIRST_URL_REG, url_bytes);

        Self {
            regs,
            xml_blob,
            clock_origin: Instant::now(),
        }
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
