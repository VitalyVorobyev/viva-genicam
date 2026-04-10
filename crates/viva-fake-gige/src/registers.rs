//! In-memory bootstrap register map and embedded GenApi XML.

use std::collections::HashMap;
use std::net::Ipv4Addr;

/// Bootstrap register addresses (GigE Vision specification).
pub const CCP: u64 = 0x0a00;
pub const HEARTBEAT_TIMEOUT: u64 = 0x0938;
pub const STREAM_CHANNEL_BASE: u64 = 0x0d00;
pub const SCP_HOST_PORT: u64 = 0x00;
pub const SCP_PACKET_SIZE: u64 = 0x04;
pub const SCP_PACKET_DELAY: u64 = 0x08;
pub const SCP_DEST_ADDR: u64 = 0x18;

/// Address where the XML file register points to.
pub const XML_ADDRESS: u64 = 0x0200;
/// Bootstrap register pointing to the XML address.
pub const XML_ADDRESS_REG: u64 = 0x0200;
/// Length of the XML URL register (bytes), contains the GenICam XML URL string.
/// The URL format is: "Local:filename.xml;ADDR;LEN"
pub const XML_URL_REG: u64 = 0x0200;

/// First XML file register address (GigE Vision spec).
pub const FIRST_URL_REG: u64 = 0x0200;
/// Length of the URL register block.
pub const URL_REG_LEN: usize = 512;

/// Address where the actual XML blob is stored in the register space.
pub const XML_BLOB_BASE: u64 = 0x1_0000;

/// Minimal GenApi XML describing a fake camera with basic SFNC features.
pub const FAKE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RegisterDescription
  ModelName="FakeGigE"
  VendorName="genicam-rs"
  ToolTip="Fake GigE Vision camera for testing"
  StandardNameSpace="GEV"
  SchemaMajorVersion="1"
  SchemaMinorVersion="1"
  SchemaSubMinorVersion="0"
  MajorVersion="1"
  MinorVersion="0"
  SubMinorVersion="0"
  ProductGuid="00000000-0000-0000-0000-000000000000"
  VersionGuid="00000000-0000-0000-0000-000000000001">

  <Category Name="Root" NameSpace="Standard">
    <pFeature>Width</pFeature>
    <pFeature>Height</pFeature>
    <pFeature>PixelFormat</pFeature>
    <pFeature>ExposureTime</pFeature>
    <pFeature>Gain</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
  </Category>

  <Integer Name="Width" NameSpace="Standard">
    <ToolTip>Image width in pixels</ToolTip>
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
    <ToolTip>Image height in pixels</ToolTip>
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

  <Enumeration Name="PixelFormat" NameSpace="Standard">
    <ToolTip>Pixel format of the image</ToolTip>
    <EnumEntry Name="Mono8" NameSpace="Standard"><Value>0x01080001</Value></EnumEntry>
    <EnumEntry Name="RGB8" NameSpace="Standard"><Value>0x02180014</Value></EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>
  <IntReg Name="PixelFormatReg"><Address>0x20008</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>

  <Float Name="ExposureTime" NameSpace="Standard">
    <ToolTip>Exposure time in microseconds</ToolTip>
    <Address>0x20010</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>10.0</Min>
    <Max>1000000.0</Max>
    <Endianess>BigEndian</Endianess>
  </Float>

  <Float Name="Gain" NameSpace="Standard">
    <ToolTip>Gain in dB</ToolTip>
    <Address>0x20018</Address>
    <Length>8</Length>
    <AccessMode>RW</AccessMode>
    <Min>0.0</Min>
    <Max>48.0</Max>
    <Endianess>BigEndian</Endianess>
  </Float>

  <Command Name="AcquisitionStart" NameSpace="Standard">
    <ToolTip>Start image acquisition</ToolTip>
    <Address>0x20020</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

  <Command Name="AcquisitionStop" NameSpace="Standard">
    <ToolTip>Stop image acquisition</ToolTip>
    <Address>0x20024</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <CommandValue>1</CommandValue>
    <Endianess>BigEndian</Endianess>
  </Command>

</RegisterDescription>
"#;

/// Pre-populated register store for the fake camera.
pub struct RegisterMap {
    regs: HashMap<u64, Vec<u8>>,
    xml_blob: Vec<u8>,
}

impl RegisterMap {
    /// Create a new register map with default values.
    pub fn new(width: u32, height: u32) -> Self {
        let mut regs = HashMap::new();

        // Bootstrap registers
        regs.insert(CCP, vec![0, 0, 0, 0]);
        regs.insert(HEARTBEAT_TIMEOUT, 3000u32.to_be_bytes().to_vec());

        // Stream channel 0 registers
        let base = STREAM_CHANNEL_BASE;
        regs.insert(base + SCP_HOST_PORT, vec![0, 0, 0, 0]);
        regs.insert(base + SCP_PACKET_SIZE, 1500u32.to_be_bytes().to_vec());
        regs.insert(base + SCP_PACKET_DELAY, vec![0, 0, 0, 0]);
        regs.insert(base + SCP_DEST_ADDR, vec![0, 0, 0, 0]);

        // Feature registers
        regs.insert(0x20000, width.to_be_bytes().to_vec()); // Width
        regs.insert(0x20004, height.to_be_bytes().to_vec()); // Height
        regs.insert(0x20008, 0x01080001u32.to_be_bytes().to_vec()); // PixelFormat = Mono8
        regs.insert(0x20010, 5000.0f64.to_be_bytes().to_vec()); // ExposureTime
        regs.insert(0x20018, 0.0f64.to_be_bytes().to_vec()); // Gain
        regs.insert(0x20020, vec![0, 0, 0, 0]); // AcquisitionStart
        regs.insert(0x20024, vec![0, 0, 0, 0]); // AcquisitionStop

        // Min/Max registers
        regs.insert(0x20100, 16u32.to_be_bytes().to_vec()); // WidthMin
        regs.insert(0x20104, 4096u32.to_be_bytes().to_vec()); // WidthMax
        regs.insert(0x20108, 16u32.to_be_bytes().to_vec()); // HeightMin
        regs.insert(0x2010c, 4096u32.to_be_bytes().to_vec()); // HeightMax

        let xml_blob = FAKE_XML.as_bytes().to_vec();

        // Store the XML URL string in the URL register block.
        // Format: "Local:fake.xml;ADDR;LEN\0"
        let url = format!("Local:fake.xml;{:x};{:x}\0", XML_BLOB_BASE, xml_blob.len());
        let mut url_bytes = vec![0u8; URL_REG_LEN];
        let src = url.as_bytes();
        url_bytes[..src.len()].copy_from_slice(src);
        regs.insert(FIRST_URL_REG, url_bytes);

        Self { regs, xml_blob }
    }

    /// Read `len` bytes starting at `addr`.
    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        // Check if it falls within the XML blob region.
        if addr >= XML_BLOB_BASE {
            let offset = (addr - XML_BLOB_BASE) as usize;
            if offset < self.xml_blob.len() {
                let end = (offset + len).min(self.xml_blob.len());
                let mut result = self.xml_blob[offset..end].to_vec();
                result.resize(len, 0);
                return result;
            }
        }

        // Try exact register match first.
        if let Some(data) = self.regs.get(&addr) {
            let mut result = data.clone();
            result.resize(len, 0);
            result.truncate(len);
            return result;
        }

        // Try to find a register that contains the requested range.
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
        // Try to update an existing register.
        if let Some(existing) = self.regs.get_mut(&addr) {
            let len = existing.len().min(data.len());
            existing[..len].copy_from_slice(&data[..len]);
            return;
        }

        // Try to update within an existing register.
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

        // Create new register.
        self.regs.insert(addr, data.to_vec());
    }

    /// Read the stream destination IP address.
    pub fn stream_dest_ip(&self) -> Ipv4Addr {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_DEST_ADDR, 4);
        Ipv4Addr::new(data[0], data[1], data[2], data[3])
    }

    /// Read the stream destination port.
    pub fn stream_dest_port(&self) -> u16 {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_HOST_PORT, 4);
        u16::from_be_bytes([data[2], data[3]])
    }

    /// Read the stream packet size.
    pub fn stream_packet_size(&self) -> u32 {
        let data = self.read(STREAM_CHANNEL_BASE + SCP_PACKET_SIZE, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Read the image width from feature registers.
    pub fn width(&self) -> u32 {
        let data = self.read(0x20000, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Read the image height from feature registers.
    pub fn height(&self) -> u32 {
        let data = self.read(0x20004, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }

    /// Read the pixel format code from feature registers.
    pub fn pixel_format_code(&self) -> u32 {
        let data = self.read(0x20008, 4);
        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
    }
}
