//! USB3 Vision transport layer.
//!
//! Provides device discovery, GenCP-over-USB control, bootstrap register
//! parsing, and bulk-endpoint streaming for USB3 Vision cameras.
//!
//! The `usb` feature flag enables actual USB hardware access via `nusb`, a
//! pure-Rust USB library (ADR-0032). Without it, only protocol types,
//! encoding, and mock-based testing compile.
//!
//! The modules are private; what is public is re-exported here (ADR-0030).
//! [`registers`] is the one public namespace: the bootstrap register maps,
//! the vocabulary the fake camera shares.

mod bootstrap;
mod control;
mod descriptor;
mod device;
mod discovery;
mod event;
#[cfg(feature = "usb")]
mod native;
mod status;
mod stream;
mod usb;

/// The bootstrap register maps, by offset: the ABRM, the SBRM and the SIRM,
/// with their capability bits (USB3 Vision 1.0; GenCP 1.2 for the ABRM).
pub mod registers {
    pub use crate::bootstrap::{abrm, capability, eirm, sbrm, sirm, u3vcp_capability};
}

pub use bootstrap::{Abrm, Eirm, FileFormat, ManifestEntry, Sbrm, Sirm, UsbSpeed};
pub use control::ControlChannel;
pub use descriptor::U3vInterfaceInfo;
pub use device::{U3vDevice, U3vStreamOptions};
pub use discovery::U3vDeviceInfo;
#[cfg(feature = "usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "usb")))]
pub use discovery::discover;
pub use event::{U3vEvent, U3vEventStream, parse_event_command};
#[cfg(feature = "usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "usb")))]
pub use native::NusbTransfer;
pub use status::U3vStatus;
pub use stream::{
    DEFAULT_MAX_TRANSFER_SIZE, DEFAULT_TRANSFERS_IN_FLIGHT, Leader, PAYLOAD_TYPE_CHUNK,
    PAYLOAD_TYPE_IMAGE, PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK, RawFrame, StreamLayout, Trailer,
    U3vStream, U3vStreamCounters, parse_chunks, parse_leader, parse_trailer,
};
pub use usb::{BulkIn, Completion, MockUsbTransfer, UsbTransfer};

/// The error a source is carried as when its type is not part of this crate's
/// API: the USB library's, the ZIP reader's.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Errors produced by the USB3 Vision transport layer.
///
/// A USB library's failure is the [`source`](std::error::Error::source) of
/// [`U3vError::Usb`], never flattened into text (ADR-0030).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum U3vError {
    /// A USB operation failed. The source is the USB library's error.
    #[error("usb: {0}")]
    Usb(#[source] BoxError),

    /// No device is at the bus and address enumeration reported.
    #[error("no USB device at bus {bus}, address {address}")]
    NotFound {
        /// The USB bus, as the operating system names it.
        bus: String,
        /// The device's address on it.
        address: u8,
    },

    /// A bulk transfer or control transaction timed out.
    #[error("timeout")]
    Timeout,

    /// The device is gone: unplugged, or reset by the host. Every later
    /// transfer fails the same way; the camera must be opened again once it
    /// is back (backlog UV-03).
    #[error("the device is disconnected")]
    Disconnected,

    /// An endpoint stalled: the device refused a transfer and halted it. A
    /// stream recovers by resetting the endpoint (backlog UV-02).
    #[error("endpoint {endpoint:#04x} stalled")]
    Stalled {
        /// The endpoint's address.
        endpoint: u8,
    },

    /// The transfer was cancelled before it completed, by the host.
    #[error("the transfer was cancelled")]
    Cancelled,

    /// An interface could not be claimed. The source is the USB library's
    /// error: another application, or a kernel driver, may hold it, or — on
    /// Windows — the interface has no WinUSB driver, which the hint says.
    #[error(
        "claim the {interface} interface ({number}): {source}{}",
        .hint.map_or(String::new(), |hint| format!("; {hint}"))
    )]
    Claim {
        /// Which USB3 Vision interface: `"control"`, `"stream"` or `"event"`.
        interface: &'static str,
        /// Its interface number.
        number: u8,
        /// The USB library's error.
        #[source]
        source: BoxError,
        /// What the user can do about it, when the cause is known.
        hint: Option<&'static str>,
    },

    /// GenCP-level decode error.
    #[error("gencp: {0}")]
    GenCp(#[from] viva_gencp::GenCpError),

    /// The device answered with an error status, decoded with USB3 Vision's
    /// table (ADR-0020).
    #[error("device status: {0}")]
    Status(U3vStatus),

    /// An acknowledge, leader or trailer does not have the shape GenCP and
    /// USB3 Vision give it.
    #[error("malformed {packet}: {problem}")]
    Malformed {
        /// What arrived, such as `"acknowledge"` or `"trailer"`.
        packet: &'static str,
        /// What is wrong, such as `"lacks the U3VC prefix"`.
        problem: &'static str,
    },

    /// A transfer carried another number of bytes than asked for.
    #[error("{what} is {got} bytes; expected {expected}")]
    Length {
        /// What was counted, such as `"READMEM_ACK payload"`.
        what: &'static str,
        /// The bytes asked for.
        expected: usize,
        /// The bytes that arrived.
        got: usize,
    },

    /// The device's bootstrap registers describe something this host cannot
    /// use.
    #[error("the device {0}")]
    Device(&'static str),

    /// The device's XML could not be unpacked. The source is the ZIP reader's
    /// or the UTF-8 decoder's error.
    #[error("device XML: {0}")]
    Xml(#[source] BoxError),
}

impl U3vError {
    /// A USB library's failure; `err` becomes the source.
    pub fn usb(err: impl Into<BoxError>) -> Self {
        U3vError::Usb(err.into())
    }

    /// Whether a device that is gone can fail a transfer this way without
    /// saying so: a USB library's unnamed failure, or a transfer the host
    /// ended. On macOS an unplug is `kIOReturnNotResponding`,
    /// `kIOReturnNotReady` or `kIOReturnNotAttached`, which `nusb` passes on
    /// as unknown (backlog UV-05). A timeout, a stall and anything the
    /// device answered say it is there.
    pub(crate) fn may_mean_gone(&self) -> bool {
        matches!(self, U3vError::Usb(_) | U3vError::Cancelled)
    }
}
