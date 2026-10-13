//! High-level USB3 Vision device handle.
//!
//! [`U3vDevice`] combines a control channel with parsed bootstrap registers
//! to provide register I/O, XML fetching, and stream configuration.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use crate::U3vError;
use crate::bootstrap::{
    Abrm, Eirm, FileFormat, ManifestEntry, Sbrm, Sirm, UsbSpeed, abrm, capability, sbrm, sirm,
    u3vcp_capability,
};
use crate::control::{ControlChannel, DEFAULT_TIMEOUT};
use crate::event::U3vEventStream;
use crate::stream::{
    DEFAULT_MAX_TRANSFER_SIZE, DEFAULT_TRANSFERS_IN_FLIGHT, StreamLayout, U3vStream,
};
use crate::usb::{GONE_GRACE, UsbTransfer, unless_gone};

/// How a stream reads: the payload size to use when the device does not say,
/// and the transfers it keeps queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct U3vStreamOptions {
    /// Bytes of payload in one block — from the `PayloadSize` feature, say —
    /// used only when the SIRM's Required Payload Size is 0; 0 for none.
    pub payload_size_hint: u64,
    /// The largest payload transfer to ask for
    /// ([`DEFAULT_MAX_TRANSFER_SIZE`]).
    pub max_transfer_size: u32,
    /// Transfers to keep queued ([`DEFAULT_TRANSFERS_IN_FLIGHT`]).
    pub transfers_in_flight: usize,
}

impl Default for U3vStreamOptions {
    fn default() -> Self {
        Self {
            payload_size_hint: 0,
            max_transfer_size: DEFAULT_MAX_TRANSFER_SIZE,
            transfers_in_flight: DEFAULT_TRANSFERS_IN_FLIGHT,
        }
    }
}

/// ZIP local file header magic: `PK\x03\x04`.
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// Extract the first file of a ZIP archive.
fn unzip_first(data: &[u8]) -> Result<Vec<u8>, U3vError> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| U3vError::Xml(Box::new(e)))?;
    if archive.is_empty() {
        return Err(U3vError::Device("serves an empty XML ZIP archive"));
    }
    let mut file = archive
        .by_index(0)
        .map_err(|e| U3vError::Xml(Box::new(e)))?;
    let mut xml = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut xml)
        .map_err(|e| U3vError::Xml(Box::new(e)))?;
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
/// `T` parameter allows production use with `NusbTransfer` (the `usb`
/// feature) and testing with [`MockUsbTransfer`](crate::usb::MockUsbTransfer).
pub struct U3vDevice<T: UsbTransfer> {
    control: ControlChannel<T>,
    abrm: Abrm,
    sbrm: Sbrm,
    stream_ep: Option<u8>,
    event_ep: Option<u8>,
    speed: Option<UsbSpeed>,
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
        // The host's word for the link, else the device's own register.
        let speed = control.transport().link_speed().or(sbrm.current_speed);
        if let Some(speed) = speed.filter(|s| s.is_below_superspeed()) {
            // Backlog UV-03: a USB 2 port, cable or hub; streaming at a
            // fraction of the camera's bandwidth, if at all.
            tracing::warn!(
                %speed,
                model = %abrm.model_name,
                "the USB3 Vision camera runs below SuperSpeed: plug it into a USB 3 port, \
                 directly or through a USB 3 hub and cable"
            );
        }

        Ok(Self {
            control,
            abrm,
            sbrm,
            stream_ep,
            event_ep,
            speed,
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

    /// The bus speed the device runs at: as the host's USB stack reports
    /// it, or else as the SBRM's current speed register says.
    pub fn current_speed(&self) -> Option<UsbSpeed> {
        self.speed
    }

    /// The device time, in ns: latched, then read from the GenCP timestamp
    /// registers (GenCP 1.3.1 §5.4.16-17); `None` when the device has none
    /// (Device Capability bit 3). The clock a stream's leaders and events
    /// are stamped with.
    pub fn timestamp(&mut self) -> Result<Option<u64>, U3vError> {
        if self.abrm.device_capability & capability::TIMESTAMP == 0 {
            return Ok(None);
        }
        self.control
            .write_mem(abrm::TIMESTAMP_LATCH, &1u32.to_le_bytes())?;
        let bytes = self.control.read_mem(abrm::TIMESTAMP, 8)?;
        Ok(Some(u64::from_le_bytes(bytes[..8].try_into().unwrap())))
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
            return Err(U3vError::Device(
                "has no streaming interface (SIRM address 0)",
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
        String::from_utf8(xml_bytes).map_err(|e| U3vError::Xml(Box::new(e)))
    }

    /// The bulk IN endpoint for streaming data, if the device has one.
    pub fn stream_ep(&self) -> Option<u8> {
        self.stream_ep
    }

    /// The bulk IN endpoint for events, if the device has one.
    pub fn event_ep(&self) -> Option<u8> {
        self.event_ep
    }

    /// [`open_stream_with`](Self::open_stream_with) the default transfer
    /// settings and `payload_hint`.
    pub fn open_stream(&mut self, payload_hint: u64) -> Result<U3vStream, U3vError>
    where
        T: 'static,
    {
        self.open_stream_with(&U3vStreamOptions {
            payload_size_hint: payload_hint,
            ..U3vStreamOptions::default()
        })
    }

    /// Configure the SIRM, reset the stream endpoint and enable streaming,
    /// returning a [`U3vStream`] that reads with the configured transfer
    /// layout.
    ///
    /// The payload size is the SIRM's Required Payload Size: the device's
    /// own figure for this stream channel. The options' hint — from the
    /// `PayloadSize` feature, say — is used only when the device leaves that
    /// register at 0. Every transfer is a multiple of the SIRM's alignment
    /// and of the endpoint's packet size, whichever is larger.
    ///
    /// The endpoint is reset — halted and cleared, so the device drops
    /// whatever an earlier stream left on it — after the layout is written
    /// and before StreamEnable is set, as aravis does
    /// (`arv_uv_device_reset_stream_endpoint`; backlog TC-27 (3)).
    pub fn open_stream_with(&mut self, options: &U3vStreamOptions) -> Result<U3vStream, U3vError>
    where
        T: 'static,
    {
        let ep = self
            .stream_ep
            .ok_or(U3vError::Device("has no streaming endpoint"))?;

        let sirm = self.read_sirm()?;
        let payload_hint = options.payload_size_hint;
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
        let transport = Arc::clone(self.control.transport());
        let packet = transport
            .max_packet_size(ep)
            .and_then(|size| u32::try_from(size).ok())
            .unwrap_or(1);
        let alignment = sirm.alignment()?.max(packet);
        let layout = StreamLayout::new(
            payload_size,
            sirm.required_leader_size,
            sirm.required_trailer_size,
            alignment,
            options.max_transfer_size,
        )?;
        let mut queue = transport.bulk_in(ep)?;
        sirm.configure(&mut self.control, &layout)?;
        queue
            .reset()
            .map_err(|error| unless_gone(error, || queue.is_present(), GONE_GRACE))?;
        sirm.enable(&mut self.control)?;

        let mut stream = U3vStream::new(queue, layout);
        stream.set_transfers_in_flight(options.transfers_in_flight);
        Ok(stream)
    }

    /// Clear StreamEnable: the device stops sending on the stream endpoint.
    pub fn stop_stream(&mut self) -> Result<(), U3vError> {
        let sirm = self.read_sirm()?;
        sirm.disable(&mut self.control)
    }

    /// The device's event interface registers; an error when it has none
    /// (U3VCP Capability bit 1, or an EIRM address of 0).
    pub fn read_eirm(&mut self) -> Result<Eirm, U3vError> {
        if self.sbrm.eirm_address == 0
            || self.sbrm.u3vcp_capability & u3vcp_capability::EIRM_AVAILABLE == 0
        {
            return Err(U3vError::Device("has no event interface (EIRM)"));
        }
        Eirm::read_from(&mut self.control, self.sbrm.eirm_address)
    }

    /// Set EventEnable and return a [`U3vEventStream`] reading the event
    /// endpoint (backlog TC-27 (4)). Which events the device sends is the
    /// XML's `EventSelector` / `EventNotification` to choose.
    pub fn open_events(&mut self) -> Result<U3vEventStream, U3vError>
    where
        T: 'static,
    {
        let ep = self
            .event_ep
            .ok_or(U3vError::Device("has no event endpoint"))?;
        let eirm = self.read_eirm()?;
        let transport = Arc::clone(self.control.transport());
        let mut queue = transport.bulk_in(ep)?;
        // As for the stream: whatever an earlier session left queued on the
        // endpoint is dropped before events are enabled.
        queue
            .reset()
            .map_err(|error| unless_gone(error, || queue.is_present(), GONE_GRACE))?;
        eirm.set_enabled(&mut self.control, true)?;
        Ok(U3vEventStream::new(
            queue,
            eirm.maximum_event_transfer_length,
            transport.max_packet_size(ep),
        ))
    }

    /// Clear EventEnable: the device stops sending events.
    pub fn stop_events(&mut self) -> Result<(), U3vError> {
        let eirm = self.read_eirm()?;
        eirm.set_enabled(&mut self.control, false)
    }

    /// The bootstrap registers as read, decoded and raw, for a diagnostic
    /// report. One camera has been checked against this (backlog TC-27), so
    /// the raw SBRM and SIRM bytes are what settles a disagreement. A register the device refuses is listed as such.
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
// Open a discovered device over nusb (convenience)
// ---------------------------------------------------------------------------

#[cfg(feature = "usb")]
impl U3vDevice<crate::native::NusbTransfer> {
    /// Open a USB3 Vision device from a [`U3vDeviceInfo`](crate::discovery::U3vDeviceInfo)
    /// obtained via [`discover()`](crate::discovery::discover).
    pub fn open_device(info: &crate::discovery::U3vDeviceInfo) -> Result<Self, U3vError> {
        let transport = crate::native::NusbTransfer::open(info)?;
        let iface = transport.interfaces().clone();
        Self::open(
            Arc::new(transport),
            iface.control_ep_in,
            iface.control_ep_out,
            iface.stream_ep_in,
            iface.event_ep_in,
        )
    }
}
