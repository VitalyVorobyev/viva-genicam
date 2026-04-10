//! High-level USB3 Vision device handle.
//!
//! [`U3vDevice`] combines a control channel with parsed bootstrap registers
//! to provide register I/O, XML fetching, and stream configuration.

use std::sync::Arc;

use crate::U3vError;
use crate::bootstrap::{Abrm, ManifestEntry, Sbrm, Sirm};
use crate::control::ControlChannel;
use crate::usb::UsbTransfer;

/// Default maximum command/ack transfer size used before reading SBRM.
///
/// The USB3 Vision spec guarantees at least 1024 bytes for the initial
/// bootstrap reads.
const INITIAL_MAX_TRANSFER: u32 = 1024;

/// High-level handle for a USB3 Vision device.
///
/// Wraps a [`ControlChannel`] and parsed bootstrap registers. The generic
/// `T` parameter allows production use with [`RusbTransfer`](crate::usb::RusbTransfer)
/// and testing with [`MockUsbTransfer`](crate::usb::MockUsbTransfer).
pub struct U3vDevice<T: UsbTransfer> {
    control: ControlChannel<T>,
    abrm: Abrm,
    sbrm: Sbrm,
    stream_ep: Option<u8>,
    event_ep: Option<u8>,
}

impl<T: UsbTransfer> U3vDevice<T> {
    /// Open a U3V device given a shared USB transport and endpoint addresses.
    ///
    /// Reads the ABRM and SBRM bootstrap registers, then re-creates the
    /// control channel with the device-reported maximum transfer sizes.
    pub fn open(
        transport: Arc<T>,
        ep_in: u8,
        ep_out: u8,
        stream_ep: Option<u8>,
        event_ep: Option<u8>,
    ) -> Result<Self, U3vError> {
        // Bootstrap with conservative transfer limits.
        let mut control = ControlChannel::new(
            Arc::clone(&transport),
            ep_in,
            ep_out,
            INITIAL_MAX_TRANSFER,
            INITIAL_MAX_TRANSFER,
        );

        let abrm = Abrm::read_from(&mut control)?;
        let sbrm = Sbrm::read_from(&mut control, abrm.sbrm_address)?;

        // Re-create the control channel with the actual device limits.
        let control = ControlChannel::new(
            transport,
            ep_in,
            ep_out,
            sbrm.max_cmd_transfer,
            sbrm.max_ack_transfer,
        );

        Ok(Self {
            control,
            abrm,
            sbrm,
            stream_ep,
            event_ep,
        })
    }

    /// Reference to the parsed ABRM.
    pub fn abrm(&self) -> &Abrm {
        &self.abrm
    }

    /// Reference to the parsed SBRM.
    pub fn sbrm(&self) -> &Sbrm {
        &self.sbrm
    }

    /// Read `len` bytes from device memory at `addr`.
    pub fn read_mem(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, U3vError> {
        self.control.read_mem(addr, len)
    }

    /// Write `data` to device memory at `addr`.
    pub fn write_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), U3vError> {
        self.control.write_mem(addr, data)
    }

    /// Read the SIRM (Streaming Interface Register Map) from the device.
    pub fn read_sirm(&mut self) -> Result<Sirm, U3vError> {
        Sirm::read_from(&mut self.control, self.sbrm.sirm_address)
    }

    /// Fetch the GenICam XML descriptor from the device's manifest table.
    ///
    /// Returns the raw XML as a string. The XML may be zipped; callers
    /// should check the first bytes for a ZIP signature and decompress
    /// if needed (handled at a higher layer).
    pub fn fetch_xml(&mut self) -> Result<String, U3vError> {
        let entry = ManifestEntry::read_first(&mut self.control, self.abrm.manifest_table_address)?;
        let raw = self
            .control
            .read_mem(entry.file_address, entry.file_size as usize)?;
        String::from_utf8(raw)
            .map_err(|e| U3vError::Protocol(format!("XML is not valid UTF-8: {e}")))
    }

    /// The bulk IN endpoint for streaming data, if the device has one.
    pub fn stream_ep(&self) -> Option<u8> {
        self.stream_ep
    }

    /// The bulk IN endpoint for events, if the device has one.
    pub fn event_ep(&self) -> Option<u8> {
        self.event_ep
    }

    /// Read the SIRM, configure stream sizes, and create a [`crate::stream::U3vStream`]
    /// for receiving frames.
    ///
    /// The stream endpoint must have been discovered during device open.
    /// Configures the SIRM with the device's maximum leader/trailer sizes
    /// and the specified payload size, then enables streaming.
    pub fn open_stream(
        &mut self,
        payload_size: u64,
    ) -> Result<crate::stream::U3vStream<T>, U3vError> {
        let ep = self
            .stream_ep
            .ok_or_else(|| U3vError::Protocol("device has no streaming endpoint".into()))?;

        let sirm = self.read_sirm()?;
        sirm.configure(
            &mut self.control,
            payload_size,
            sirm.max_leader_size,
            sirm.max_trailer_size,
        )?;
        sirm.enable(&mut self.control)?;

        Ok(crate::stream::U3vStream::new(
            self.control.transport().clone(),
            ep,
            sirm.max_leader_size as usize,
            sirm.max_trailer_size as usize,
            payload_size as usize,
        ))
    }

    /// Disable streaming via the SIRM control register.
    pub fn stop_stream(&mut self) -> Result<(), U3vError> {
        let sirm = self.read_sirm()?;
        sirm.disable(&mut self.control)
    }
}

// ---------------------------------------------------------------------------
// Open from rusb device (convenience)
// ---------------------------------------------------------------------------

#[cfg(feature = "usb")]
impl U3vDevice<crate::usb::RusbTransfer> {
    /// Open a USB3 Vision device from a [`U3vDeviceInfo`](crate::discovery::U3vDeviceInfo)
    /// obtained via [`discover()`](crate::discovery::discover).
    pub fn open_device(info: &crate::discovery::U3vDeviceInfo) -> Result<Self, U3vError> {
        use rusb::UsbContext;
        let context = rusb::Context::new().map_err(|e| U3vError::Usb(e.to_string()))?;
        let devices = context
            .devices()
            .map_err(|e| U3vError::Usb(e.to_string()))?;

        let device = devices
            .iter()
            .find(|d| d.bus_number() == info.bus && d.address() == info.address)
            .ok_or_else(|| {
                U3vError::Usb(format!(
                    "device not found at bus={} addr={}",
                    info.bus, info.address
                ))
            })?;

        let handle = device.open().map_err(|e| U3vError::Usb(e.to_string()))?;

        // Claim all U3V interfaces.
        let iface = &info.interface_info;
        handle
            .claim_interface(iface.control_iface)
            .map_err(|e| U3vError::Usb(format!("claim control interface: {e}")))?;
        if let Some(si) = iface.stream_iface {
            let _ = handle.claim_interface(si);
        }
        if let Some(ei) = iface.event_iface {
            let _ = handle.claim_interface(ei);
        }

        let transport = Arc::new(crate::usb::RusbTransfer::new(Arc::new(handle)));
        Self::open(
            transport,
            iface.control_ep_in,
            iface.control_ep_out,
            iface.stream_ep_in,
            iface.event_ep_in,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::MockUsbTransfer;
    use bytes::{BufMut, BytesMut};

    const EP_OUT: u8 = 0x01;
    const EP_IN: u8 = 0x81;
    const ACK_PREFIX_LE: u32 = 0x4356_3341;
    const PREFIX_SIZE: usize = 12;

    fn success_ack(request_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = BytesMut::with_capacity(PREFIX_SIZE + payload.len());
        buf.put_u32_le(ACK_PREFIX_LE);
        buf.put_u16_le(0x0000); // Success
        buf.put_u16_le(0x0085); // ReadMem ack
        buf.put_u16_le(payload.len() as u16);
        buf.put_u16_le(request_id);
        buf.extend_from_slice(payload);
        buf.to_vec()
    }

    /// Enqueue all the ack responses needed for Abrm::read_from + Sbrm::read_from.
    /// Returns the next request_id.
    fn enqueue_bootstrap_responses(mock: &MockUsbTransfer, sbrm_addr: u64) -> u16 {
        let mut req: u16 = 0;

        // -- ABRM reads --
        // gencp_version (4 bytes)
        mock.enqueue_read(EP_IN, success_ack(req, &0x0001_0000u32.to_be_bytes()));
        req += 1;
        // manufacturer_name (64 bytes)
        let mut s = vec![0u8; 64];
        s[..4].copy_from_slice(b"Test");
        mock.enqueue_read(EP_IN, success_ack(req, &s));
        req += 1;
        // model_name
        s = vec![0u8; 64];
        s[..7].copy_from_slice(b"MockCam");
        mock.enqueue_read(EP_IN, success_ack(req, &s));
        req += 1;
        // family_name
        mock.enqueue_read(EP_IN, success_ack(req, &[0u8; 64]));
        req += 1;
        // device_version
        mock.enqueue_read(EP_IN, success_ack(req, &[0u8; 64]));
        req += 1;
        // serial_number
        mock.enqueue_read(EP_IN, success_ack(req, &[0u8; 64]));
        req += 1;
        // user_defined_name
        mock.enqueue_read(EP_IN, success_ack(req, &[0u8; 64]));
        req += 1;
        // manifest_table_address (8 bytes)
        mock.enqueue_read(EP_IN, success_ack(req, &0x0005_0000u64.to_be_bytes()));
        req += 1;
        // sbrm_address (8 bytes)
        mock.enqueue_read(EP_IN, success_ack(req, &sbrm_addr.to_be_bytes()));
        req += 1;
        // device_capability (8 bytes)
        mock.enqueue_read(EP_IN, success_ack(req, &0u64.to_be_bytes()));
        req += 1;

        // -- SBRM reads --
        // u3v_version
        mock.enqueue_read(EP_IN, success_ack(req, &0x0001_0000u32.to_be_bytes()));
        req += 1;
        // max_cmd_transfer
        mock.enqueue_read(EP_IN, success_ack(req, &2048u32.to_be_bytes()));
        req += 1;
        // max_ack_transfer
        mock.enqueue_read(EP_IN, success_ack(req, &2048u32.to_be_bytes()));
        req += 1;
        // num_stream_channels
        mock.enqueue_read(EP_IN, success_ack(req, &1u32.to_be_bytes()));
        req += 1;
        // sirm_address
        mock.enqueue_read(EP_IN, success_ack(req, &0x0002_0000u64.to_be_bytes()));
        req += 1;
        // sirm_length
        mock.enqueue_read(EP_IN, success_ack(req, &256u32.to_be_bytes()));
        req += 1;
        // eirm_address
        mock.enqueue_read(EP_IN, success_ack(req, &0x0003_0000u64.to_be_bytes()));
        req += 1;
        // eirm_length
        mock.enqueue_read(EP_IN, success_ack(req, &64u32.to_be_bytes()));
        req += 1;

        req
    }

    #[test]
    fn open_device_reads_bootstrap() {
        let mock = Arc::new(MockUsbTransfer::new());
        let sbrm_addr: u64 = 0x0001_0000;
        enqueue_bootstrap_responses(&mock, sbrm_addr);

        let dev = U3vDevice::open(Arc::clone(&mock), EP_IN, EP_OUT, Some(0x82), None).unwrap();

        assert_eq!(dev.abrm().manufacturer_name, "Test");
        assert_eq!(dev.sbrm().max_cmd_transfer, 2048);
        assert_eq!(dev.sbrm().max_ack_transfer, 2048);
        assert_eq!(dev.sbrm().num_stream_channels, 1);
        assert_eq!(dev.stream_ep(), Some(0x82));
        assert_eq!(dev.event_ep(), None);
    }

    #[test]
    fn fetch_xml_from_manifest() {
        let mock = Arc::new(MockUsbTransfer::new());
        let sbrm_addr: u64 = 0x0001_0000;
        enqueue_bootstrap_responses(&mock, sbrm_addr);

        let mut dev = U3vDevice::open(Arc::clone(&mock), EP_IN, EP_OUT, None, None).unwrap();

        // After open(), the control channel is re-created with request_id = 0.
        let mut req: u16 = 0;

        // Manifest table: count = 1
        mock.enqueue_read(EP_IN, success_ack(req, &1u32.to_be_bytes()));
        req += 1;

        // Manifest entry: [8 info][8 addr][8 size]
        let xml_addr: u64 = 0x0010_0000;
        let xml_content = b"<RegisterDescription />";
        let xml_size = xml_content.len() as u64;
        let mut entry_data = BytesMut::with_capacity(24);
        entry_data.put_u64(0); // file info
        entry_data.put_u64(xml_addr);
        entry_data.put_u64(xml_size);
        mock.enqueue_read(EP_IN, success_ack(req, &entry_data));
        req += 1;

        // XML content
        mock.enqueue_read(EP_IN, success_ack(req, xml_content));

        let xml = dev.fetch_xml().unwrap();
        assert_eq!(xml, "<RegisterDescription />");
    }
}
