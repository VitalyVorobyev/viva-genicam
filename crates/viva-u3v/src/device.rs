//! High-level USB3 Vision device handle.
//!
//! [`U3vDevice`] combines a control channel with parsed bootstrap registers
//! to provide register I/O, XML fetching, and stream configuration.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::U3vError;
use crate::bootstrap::{Abrm, FileFormat, ManifestEntry, Sbrm, Sirm, sbrm, sirm};
use crate::control::{ControlChannel, DEFAULT_TIMEOUT};
use crate::stream::{DEFAULT_MAX_TRANSFER_SIZE, StreamLayout, U3vStream};
use crate::usb::UsbTransfer;

/// ZIP local file header magic: `PK\x03\x04`.
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// Extract the first file of a ZIP archive.
fn unzip_first(data: &[u8]) -> Result<Vec<u8>, U3vError> {
    let cursor = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| U3vError::Protocol(format!("bad ZIP: {e}")))?;
    if archive.is_empty() {
        return Err(U3vError::Protocol("ZIP archive is empty".into()));
    }
    let mut file = archive
        .by_index(0)
        .map_err(|e| U3vError::Protocol(format!("cannot read ZIP entry: {e}")))?;
    let mut xml = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut xml)
        .map_err(|e| U3vError::Protocol(format!("ZIP decompression failed: {e}")))?;
    Ok(xml)
}

/// Transfer limit used for the bootstrap reads, before the SBRM gives the
/// device's own: small enough for any device.
const INITIAL_MAX_TRANSFER: u32 = 1024;

/// Added to the device's Maximum Device Response Time, which counts only
/// the command's execution, for the two transfers around it (GenCP 1.2
/// §3.1.2).
const TRANSFER_MARGIN: Duration = Duration::from_millis(100);

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
    /// Reads the ABRM and SBRM bootstrap registers, then uses the device's
    /// response time and transfer limits on the same control channel.
    pub fn open(
        transport: Arc<T>,
        ep_in: u8,
        ep_out: u8,
        stream_ep: Option<u8>,
        event_ep: Option<u8>,
    ) -> Result<Self, U3vError> {
        let mut control = ControlChannel::new(
            transport,
            ep_in,
            ep_out,
            INITIAL_MAX_TRANSFER,
            INITIAL_MAX_TRANSFER,
        );

        let abrm = Abrm::read_from(&mut control)?;
        let response_time =
            Duration::from_millis(u64::from(abrm.max_device_response_time_ms)) + TRANSFER_MARGIN;
        control.set_timeout(response_time.max(DEFAULT_TIMEOUT));

        let sbrm = Sbrm::read_from(&mut control, abrm.sbrm_address)?;
        control.set_max_transfer(sbrm.max_cmd_transfer, sbrm.max_ack_transfer);

        Ok(Self {
            control,
            abrm,
            sbrm,
            stream_ep,
            event_ep,
        })
    }

    /// Access the shared transport handle (e.g. to create additional streams).
    pub fn transport(&self) -> Arc<T> {
        self.control.transport().clone()
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

    /// Read the device's half of the SIRM (Streaming Interface Register Map).
    pub fn read_sirm(&mut self) -> Result<Sirm, U3vError> {
        if self.sbrm.sirm_address == 0 {
            return Err(U3vError::Protocol(
                "the device has no streaming interface (SIRM address 0)".into(),
            ));
        }
        Sirm::read_from(&mut self.control, self.sbrm.sirm_address)
    }

    /// Fetch the GenICam XML descriptor from the device's manifest table.
    ///
    /// A ZIP-compressed file — by the manifest's file format, or by its
    /// `PK\x03\x04` signature — is decompressed. Returns the XML as a UTF-8
    /// string.
    pub fn fetch_xml(&mut self) -> Result<String, U3vError> {
        let entry =
            ManifestEntry::read_device_xml(&mut self.control, self.abrm.manifest_table_address)?;
        let raw = self
            .control
            .read_mem(entry.file_address, entry.file_size as usize)?;
        let zipped = entry.format == FileFormat::Zip || raw.starts_with(ZIP_MAGIC);
        let xml_bytes = if zipped { unzip_first(&raw)? } else { raw };
        String::from_utf8(xml_bytes)
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

    /// Configure the SIRM and enable streaming, returning a
    /// [`U3vStream`] that reads with the configured transfer layout.
    ///
    /// The payload size is the SIRM's Required Payload Size: the device's
    /// own figure for this stream channel. `payload_hint` — from the
    /// `PayloadSize` feature, say — is used only when the device leaves that
    /// register at 0.
    pub fn open_stream(&mut self, payload_hint: u64) -> Result<U3vStream<T>, U3vError> {
        let ep = self
            .stream_ep
            .ok_or_else(|| U3vError::Protocol("device has no streaming endpoint".into()))?;

        let sirm = self.read_sirm()?;
        let payload_size = match sirm.required_payload_size {
            0 => payload_hint,
            required => {
                if payload_hint != 0 && payload_hint != required {
                    tracing::debug!(
                        required,
                        payload_hint,
                        "SIRM requires a payload size other than the hint; using the SIRM's"
                    );
                }
                required
            }
        };
        let layout = StreamLayout::new(
            payload_size,
            sirm.required_leader_size,
            sirm.required_trailer_size,
            sirm.alignment()?,
            DEFAULT_MAX_TRANSFER_SIZE,
        )?;
        sirm.configure(&mut self.control, &layout)?;
        sirm.enable(&mut self.control)?;

        Ok(U3vStream::new(self.control.transport().clone(), ep, layout))
    }

    /// Clear StreamEnable: the device stops sending on the stream endpoint.
    pub fn stop_stream(&mut self) -> Result<(), U3vError> {
        let sirm = self.read_sirm()?;
        sirm.disable(&mut self.control)
    }

    /// The bootstrap registers as read, decoded and raw, for a diagnostic
    /// report. Nothing about USB3 Vision has been checked against a camera
    /// (backlog TC-27), so the raw SBRM and SIRM bytes are what settles a
    /// disagreement. A register the device refuses is listed as such.
    pub fn describe(&mut self) -> Vec<(String, String)> {
        let abrm = &self.abrm;
        let sbrm = &self.sbrm;
        let version = |v: u32| format!("{}.{}", v >> 16, v & 0xFFFF);
        let mut lines = vec![
            ("GenCP version".to_string(), version(abrm.gencp_version)),
            (
                "device capability".to_string(),
                format!("{:#018x}", abrm.device_capability),
            ),
            (
                "max device response time".to_string(),
                format!("{} ms", abrm.max_device_response_time_ms),
            ),
            (
                "manifest table".to_string(),
                format!("{:#x}", abrm.manifest_table_address),
            ),
            ("SBRM".to_string(), format!("{:#x}", sbrm.base)),
            ("U3V version".to_string(), version(sbrm.u3v_version)),
            (
                "U3VCP capability".to_string(),
                format!("{:#018x}", sbrm.u3vcp_capability),
            ),
            (
                "max command / ack transfer".to_string(),
                format!(
                    "{} / {} bytes",
                    sbrm.max_cmd_transfer, sbrm.max_ack_transfer
                ),
            ),
            (
                "stream channels".to_string(),
                sbrm.num_stream_channels.to_string(),
            ),
            (
                "SIRM".to_string(),
                format!("{:#x} ({} bytes)", sbrm.sirm_address, sbrm.sirm_length),
            ),
        ];
        let sbrm_base = sbrm.base;
        let sirm_base = sbrm.sirm_address;
        lines.push((
            "SBRM bytes".to_string(),
            self.hex(sbrm_base, sbrm::LEN as usize),
        ));
        if sirm_base != 0 {
            lines.push((
                "SIRM bytes".to_string(),
                self.hex(sirm_base, sirm::LEN as usize),
            ));
        }
        lines
    }

    fn hex(&mut self, addr: u64, len: usize) -> String {
        match self.control.read_mem(addr, len) {
            Ok(bytes) => bytes.iter().fold(String::new(), |mut out, b| {
                let _ = write!(out, "{b:02x}");
                out
            }),
            Err(e) => format!("unreadable: {e}"),
        }
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
