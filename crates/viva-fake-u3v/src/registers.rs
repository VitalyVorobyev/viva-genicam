//! In-memory register map and embedded GenApi XML for the fake U3V camera.

use std::collections::HashMap;

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
pub const REG_DEVICE_MODEL: u64 = 0x20200;
pub const REG_DEVICE_VENDOR: u64 = 0x20220;

/// XML blob address in the register space.
pub const XML_BLOB_BASE: u64 = 0x1_0000;

// ABRM register offsets (GenCP standard, base address 0x0000).
pub const ABRM_GENCP_VERSION: u64 = 0x0000;
pub const ABRM_MANUFACTURER: u64 = 0x0048;
pub const ABRM_MODEL: u64 = 0x0088;
pub const ABRM_FAMILY: u64 = 0x00C8;
pub const ABRM_DEVICE_VERSION: u64 = 0x0108;
pub const ABRM_SERIAL: u64 = 0x01A8;
pub const ABRM_USER_NAME: u64 = 0x01E8;
pub const ABRM_MANIFEST_TABLE: u64 = 0x0228;
pub const ABRM_SBRM_ADDRESS: u64 = 0x0230;
pub const ABRM_DEVICE_CAPABILITY: u64 = 0x0238;

// SBRM address.
pub const SBRM_BASE: u64 = 0x1_0000_0000;
// SIRM address.
pub const SIRM_BASE: u64 = 0x2_0000_0000;
// Manifest table address.
pub const MANIFEST_TABLE: u64 = 0x3_0000_0000;

/// In-memory register map.
pub struct RegisterMap {
    regs: HashMap<u64, Vec<u8>>,
}

impl RegisterMap {
    pub fn new(width: u32, height: u32, pixel_format: u32) -> Self {
        let xml = generate_xml();
        let mut map = Self {
            regs: HashMap::new(),
        };

        // -- ABRM (at address 0x0000) --
        map.write_u32(ABRM_GENCP_VERSION, 0x0001_0000);
        map.write_string(ABRM_MANUFACTURER, "FakeCorp", 64);
        map.write_string(ABRM_MODEL, "FakeU3V", 64);
        map.write_string(ABRM_FAMILY, "Test", 64);
        map.write_string(ABRM_DEVICE_VERSION, "1.0.0", 64);
        map.write_string(ABRM_SERIAL, "FAKE-001", 64);
        map.write_string(ABRM_USER_NAME, "", 64);
        map.write_u64(ABRM_MANIFEST_TABLE, MANIFEST_TABLE);
        map.write_u64(ABRM_SBRM_ADDRESS, SBRM_BASE);
        map.write_u64(ABRM_DEVICE_CAPABILITY, 0);

        // -- SBRM --
        map.write_u32(SBRM_BASE, 0x0001_0000); // u3v_version
        map.write_u32(SBRM_BASE + 0x0004, 4096); // max_cmd_transfer
        map.write_u32(SBRM_BASE + 0x0008, 4096); // max_ack_transfer
        map.write_u32(SBRM_BASE + 0x000C, 1); // num_stream_channels
        map.write_u64(SBRM_BASE + 0x0010, SIRM_BASE); // sirm_address
        map.write_u32(SBRM_BASE + 0x0018, 256); // sirm_length
        map.write_u64(SBRM_BASE + 0x001C, 0); // eirm_address
        map.write_u32(SBRM_BASE + 0x0024, 0); // eirm_length

        // -- SIRM --
        map.write_u32(SIRM_BASE, 0); // info
        map.write_u32(SIRM_BASE + 0x0004, 0); // control
        map.write_u64(SIRM_BASE + 0x0008, 0); // req_payload_size
        map.write_u32(SIRM_BASE + 0x0010, 0); // req_leader_size
        map.write_u32(SIRM_BASE + 0x0014, 0); // req_trailer_size
        map.write_u32(SIRM_BASE + 0x0018, 256); // max_leader_size
        map.write_u32(SIRM_BASE + 0x001C, 256); // max_trailer_size
        map.write_u64(SIRM_BASE + 0x0020, 0); // payload_size
        map.write_u32(SIRM_BASE + 0x0028, 0); // payload_count
        map.write_u32(SIRM_BASE + 0x002C, 0); // transfer1_size
        map.write_u32(SIRM_BASE + 0x0030, 0); // transfer2_size
        map.write_u32(SIRM_BASE + 0x0034, 1024 * 1024); // max_payload_transfer

        // -- Manifest table --
        // Header: count = 1 (4 bytes) + 4 reserved
        map.write_u32(MANIFEST_TABLE, 1);
        // Entry: [8 info][8 address][8 size]
        let entry_offset = MANIFEST_TABLE + 8;
        map.write_u64(entry_offset, 0); // file info
        map.write_u64(entry_offset + 8, XML_BLOB_BASE); // file address
        map.write_u64(entry_offset + 16, xml.len() as u64); // file size

        // -- XML blob --
        map.write_blob(XML_BLOB_BASE, xml.as_bytes());

        // -- Feature registers --
        map.write_u32(REG_WIDTH, width);
        map.write_u32(REG_HEIGHT, height);
        map.write_u32(REG_PIXEL_FORMAT, pixel_format);
        map.write_u32(REG_SENSOR_WIDTH, width);
        map.write_u32(REG_SENSOR_HEIGHT, height);
        map.write_u32(REG_ACQ_MODE, 0); // Continuous
        map.write_u32(REG_ACQ_FRAME_RATE, 30_f32.to_bits());
        map.write_f64(REG_EXPOSURE_TIME, 10000.0);
        map.write_string(REG_DEVICE_MODEL, "FakeU3V", 32);
        map.write_string(REG_DEVICE_VENDOR, "FakeCorp", 32);

        map
    }

    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        let mut result = vec![0u8; len];
        for (i, byte) in result.iter_mut().enumerate() {
            let a = addr + i as u64;
            // Find which register block contains this address.
            for (&base, data) in &self.regs {
                let end = base + data.len() as u64;
                if a >= base && a < end {
                    *byte = data[(a - base) as usize];
                    break;
                }
            }
        }
        result
    }

    pub fn write(&mut self, addr: u64, data: &[u8]) {
        // Write into existing register blocks or create new ones.
        for (i, &byte) in data.iter().enumerate() {
            let a = addr + i as u64;
            let mut found = false;
            for (&base, block) in self.regs.iter_mut() {
                let end = base + block.len() as u64;
                if a >= base && a < end {
                    block[(a - base) as usize] = byte;
                    found = true;
                    break;
                }
            }
            if !found {
                self.regs.insert(a, vec![byte]);
            }
        }
    }

    fn write_u32(&mut self, addr: u64, value: u32) {
        self.regs.insert(addr, value.to_be_bytes().to_vec());
    }

    fn write_u64(&mut self, addr: u64, value: u64) {
        self.regs.insert(addr, value.to_be_bytes().to_vec());
    }

    fn write_f64(&mut self, addr: u64, value: f64) {
        self.regs.insert(addr, value.to_be_bytes().to_vec());
    }

    fn write_string(&mut self, addr: u64, s: &str, max_len: usize) {
        let mut data = vec![0u8; max_len];
        let bytes = s.as_bytes();
        let n = bytes.len().min(max_len - 1);
        data[..n].copy_from_slice(&bytes[..n]);
        self.regs.insert(addr, data);
    }

    fn write_blob(&mut self, addr: u64, data: &[u8]) {
        self.regs.insert(addr, data.to_vec());
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
  </Category>

  <Category Name="DeviceControl" NameSpace="Standard">
    <pFeature>DeviceModelName</pFeature>
    <pFeature>DeviceVendorName</pFeature>
  </Category>

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
    <Endianess>BigEndian</Endianess>
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
    <Endianess>BigEndian</Endianess>
  </IntReg>

  <Enumeration Name="PixelFormat" NameSpace="Standard">
    <EnumEntry Name="Mono8" NameSpace="Standard">
      <Value>17301505</Value>
    </EnumEntry>
    <EnumEntry Name="RGB8Packed" NameSpace="Standard">
      <Value>35127316</Value>
    </EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>
  <IntReg Name="PixelFormatReg">
    <Address>0x20008</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <pPort>Device</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>

  <Category Name="AcquisitionControl" NameSpace="Standard">
    <pFeature>AcquisitionMode</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
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
    <Endianess>BigEndian</Endianess>
  </IntReg>

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
    <Endianess>BigEndian</Endianess>
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
    <Endianess>BigEndian</Endianess>
  </IntReg>

  <Port Name="Device" NameSpace="Standard" />
</RegisterDescription>"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_map_read_write_u32() {
        let map = RegisterMap::new(640, 480, 0x0108_0001);
        let width = map.read(REG_WIDTH, 4);
        assert_eq!(u32::from_be_bytes(width[..4].try_into().unwrap()), 640);
    }

    #[test]
    fn register_map_xml_accessible() {
        let map = RegisterMap::new(320, 240, 0x0108_0001);
        // Read manifest table count
        let count_bytes = map.read(MANIFEST_TABLE, 4);
        let count = u32::from_be_bytes(count_bytes[..4].try_into().unwrap());
        assert_eq!(count, 1);

        // Read XML blob start
        let xml_start = map.read(XML_BLOB_BASE, 5);
        assert_eq!(&xml_start, b"<?xml");
    }

    #[test]
    fn register_map_abrm_sbrm_address() {
        let map = RegisterMap::new(640, 480, 0x0108_0001);
        let sbrm_bytes = map.read(ABRM_SBRM_ADDRESS, 8);
        let sbrm_addr = u64::from_be_bytes(sbrm_bytes[..8].try_into().unwrap());
        assert_eq!(sbrm_addr, SBRM_BASE);
    }
}
