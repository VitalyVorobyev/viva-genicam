//! The facade's error type.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use thiserror::Error;

use crate::ControlState;
use crate::genapi::GenApiError;
use crate::genapi::persistence::ParseConfigurationError;
use crate::gige::GigeError;
use viva_genapi_xml::XmlError;
use viva_pfnc::convert::ConvertError;

/// Error type produced by the high level GenICam facade.
///
/// Every variant either carries what caused it as its
/// [`source`](std::error::Error::source) — a GVCP status, a USB failure, a
/// socket's error — or names what it refused with typed fields; nothing is
/// flattened into text (ADR-0030). The `during` of a transport variant says
/// what the facade was doing, such as `"claim control"`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GenicamError {
    /// A feature's read, write or evaluation failed in the node map.
    #[error(transparent)]
    GenApi(#[from] GenApiError),
    /// The camera's GenApi XML could not be fetched or parsed.
    #[error("GenApi XML: {0}")]
    Xml(#[from] XmlError),
    /// A GigE Vision control transaction failed.
    #[error("{during}: {source}")]
    Gige {
        /// What the facade was doing.
        during: &'static str,
        /// The GVCP error: a status, a timeout, a socket's failure.
        #[source]
        source: GigeError,
    },
    /// A USB3 Vision transaction failed.
    #[cfg(feature = "u3v")]
    #[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
    #[error("{during}: {source}")]
    U3v {
        /// What the facade was doing.
        during: &'static str,
        /// The USB3 Vision error.
        #[source]
        source: crate::u3v::U3vError,
    },
    /// A socket operation failed: binding a stream or event socket, joining
    /// a multicast group, receiving.
    #[error("{during}: {source}")]
    Network {
        /// What the facade was doing.
        during: &'static str,
        /// The socket's error.
        #[source]
        source: std::io::Error,
    },
    /// Reading or writing a file or stream failed.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// A configuration file does not parse (see
    /// [`Camera::load_features`](crate::Camera::load_features)).
    #[error("configuration file: {0}")]
    Configuration(#[from] ParseConfigurationError),
    /// A frame's pixels could not be converted.
    #[error("cannot convert the frame: {0}")]
    Convert(#[source] ConvertError),
    /// Required chunk feature missing from the nodemap.
    #[error("chunk feature '{0}' not found; verify camera supports chunk data")]
    MissingChunkFeature(String),
    /// The camera reported a pixel format without a conversion path.
    #[error("unsupported pixel format: {0}")]
    UnsupportedPixelFormat(viva_pfnc::PixelFormat),
    /// A command was written but the camera did not report it finished
    /// (GenICam `IsDone`) within the time allowed.
    ///
    /// The write itself was acknowledged: the camera may still be acting on
    /// it, or its command register may never clear.
    #[error("command {name} was executed but did not report completion within {waited:?}")]
    CommandNotDone {
        /// The command node.
        name: String,
        /// How long [`Camera::execute_and_wait`](crate::Camera::execute_and_wait)
        /// polled.
        waited: Duration,
    },
    /// Nothing arrived within the time a blocking call was given.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    /// The camera declares none of the names a standard feature goes by.
    #[error("the camera has no {what}: it declares none of {}", tried.join(", "))]
    MissingFeature {
        /// What was looked for, such as `"exposure time"`.
        what: &'static str,
        /// Every name tried, in the order tried.
        tried: &'static [&'static str],
    },
    /// Events were asked for on a camera whose XML has no
    /// `EventSelector`/`EventNotification`; event selection has no bootstrap
    /// register to fall back on.
    #[error(
        "cannot enable events {events:?}: the camera declares neither EventSelector nor \
         EventNotification, and event selection has no bootstrap register"
    )]
    NoEventSelection {
        /// The events asked for.
        events: Vec<String>,
    },
    /// The session does not hold control of the camera, so it neither
    /// writes nor configures a stream: it was opened as a monitor, or
    /// control was released or lost.
    #[error("{}", not_in_control(*.0))]
    NotInControl(ControlState),
    /// A GVSP packet size below the smallest a stream accepts.
    #[error(
        "GVSP packet size {size} is below the {min}-byte minimum; raise the camera's \
         GevSCPSPacketSize, or pass auto_packet_size() or packet_size(n)"
    )]
    PacketTooSmall {
        /// The packet size, as asked for or as the camera set it.
        size: u32,
        /// The smallest accepted.
        min: u32,
    },
    /// A GVSP packet size above the largest an IPv4 datagram carries.
    #[error("GVSP packet size {size} exceeds the {max}-byte maximum an IPv4 datagram carries")]
    PacketTooLarge {
        /// The packet size asked for.
        size: u32,
        /// The largest accepted.
        max: u32,
    },
    /// A stream channel the camera does not have; `count` is its
    /// `GevNumberOfStreamChannels`, when it says.
    #[error("{}", stream_channel(*channel, *count))]
    StreamChannel {
        /// The channel asked for.
        channel: u32,
        /// The channels the camera has.
        count: Option<u32>,
    },
    /// An inter-packet delay that cannot be expressed in `GevSCPD`'s unit.
    #[error("cannot set a packet delay of {delay:?}: {problem}")]
    PacketDelay {
        /// The delay asked for.
        delay: Duration,
        /// Why, such as `"the camera reports a tick frequency of 0"`.
        problem: &'static str,
    },
    /// A stream needs a host interface to receive on, and none was given.
    #[error("a stream needs a network interface to receive on; none was given")]
    NoInterface,
    /// The interface given has no IPv4 address.
    #[error("interface {0} has no IPv4 address")]
    InterfaceWithoutIpv4(String),
    /// No host interface routes to the camera; pass one explicitly.
    #[error("no local interface routes to {camera}: {source}")]
    NoRoute {
        /// The camera's address.
        camera: Ipv4Addr,
        /// The route probe's error.
        #[source]
        source: std::io::Error,
    },
    /// An address that is not a multicast group where one is needed: the
    /// group asked for, or where the camera streams to for a monitor.
    #[error("{0} is not a multicast group (224.0.0.0/4)")]
    NotMulticast(SocketAddrV4),
    /// Time calibration could not fit the camera's clock.
    #[error("time calibration: {0}")]
    TimeSync(&'static str),
    /// A binning enumeration entry names no factor (SVS's `On`).
    #[error("{feature}: entry {entry} names no binning factor")]
    NoBinningFactor {
        /// The binning feature.
        feature: &'static str,
        /// The entry it holds.
        entry: String,
    },
    /// No binning enumeration entry names the factor asked for.
    #[error("{feature}: no entry names a binning factor of {factor}; its entries are {}", entries.join(", "))]
    NoBinningEntry {
        /// The binning feature.
        feature: &'static str,
        /// The factor asked for.
        factor: i64,
        /// The entries it has.
        entries: Vec<String>,
    },
    /// A frame cannot be sized: the camera has no `PayloadSize`, and the
    /// pixel format declares no size.
    #[error(
        "cannot size a {pixel_format} frame: the camera has no PayloadSize and the format \
         declares no size; give the payload size explicitly"
    )]
    UnsizedFrame {
        /// The pixel format, as the camera names it.
        pixel_format: String,
    },
    /// A thread panicked: one holding the device's lock, which a panic
    /// poisons, or one the call ran on.
    #[error("{0} panicked")]
    Panicked(&'static str),
    /// A background task the call waited on panicked or was cancelled.
    #[error("{during}: {source}")]
    Task {
        /// What the task was doing.
        during: &'static str,
        /// Its join error.
        #[source]
        source: tokio::task::JoinError,
    },
    /// A blocking call was made inside an async runtime, which it would
    /// stall; use the async API there.
    #[error(
        "blocking::{0} was called inside an async runtime, which it would stall; use the async API there"
    )]
    InsideRuntime(&'static str),
}

/// The message for [`GenicamError::NotInControl`], with what to do about it.
fn not_in_control(state: ControlState) -> &'static str {
    match state {
        ControlState::Monitor => {
            "the camera was opened as a read-only monitor, so nothing is written; open it \
             with Access::Control, or reclaim_control, to write"
        }
        ControlState::Released => {
            "control of the camera was released, so nothing is written; reclaim_control \
             takes it again"
        }
        _ => {
            "control of the camera was lost, so nothing is written; reclaim_control takes it again"
        }
    }
}

fn stream_channel(channel: u32, count: Option<u32>) -> String {
    match count {
        Some(count) => format!(
            "the camera has {count} stream channel(s) (GevNumberOfStreamChannels); stream \
             channel {channel} does not exist"
        ),
        None => format!(
            "the camera does not say how many stream channels it has; only stream channel 0 \
             can be configured, not {channel}"
        ),
    }
}

impl From<GigeError> for GenicamError {
    fn from(source: GigeError) -> Self {
        GenicamError::Gige {
            during: "GigE Vision control",
            source,
        }
    }
}

#[cfg(feature = "u3v")]
impl From<crate::u3v::U3vError> for GenicamError {
    fn from(source: crate::u3v::U3vError) -> Self {
        GenicamError::U3v {
            during: "USB3 Vision control",
            source,
        }
    }
}

impl From<ConvertError> for GenicamError {
    fn from(err: ConvertError) -> Self {
        match err {
            ConvertError::Unsupported(format) => GenicamError::UnsupportedPixelFormat(format),
            other => GenicamError::Convert(other),
        }
    }
}

/// `map_err` for a GVCP call: the error, with what was being done.
pub(crate) fn gige(during: &'static str) -> impl FnOnce(GigeError) -> GenicamError {
    move |source| GenicamError::Gige { during, source }
}

/// `map_err` for a USB3 Vision call: the error, with what was being done.
#[cfg(feature = "u3v")]
pub(crate) fn u3v(during: &'static str) -> impl FnOnce(crate::u3v::U3vError) -> GenicamError {
    move |source| GenicamError::U3v { during, source }
}

/// `map_err` for a socket call: the error, with what was being done.
pub(crate) fn network(during: &'static str) -> impl FnOnce(std::io::Error) -> GenicamError {
    move |source| GenicamError::Network { during, source }
}

/// `map_err` for a task join: the error, with what the task was doing.
pub(crate) fn task(during: &'static str) -> impl FnOnce(tokio::task::JoinError) -> GenicamError {
    move |source| GenicamError::Task { during, source }
}

/// Why a [`GigeRegisterIo`](crate::GigeRegisterIo) refused a write before it
/// reached the camera: the session does not hold control. The
/// [`source`](std::error::Error::source) of the [`GenApiError::Io`] a
/// refused feature write returns, so a caller can tell it from a camera's
/// refusal by downcasting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{} (register {address:#010x})", not_in_control(*state))]
#[non_exhaustive]
pub struct WriteRefused {
    /// Where the session stands.
    pub state: ControlState,
    /// The register the write was for.
    pub address: u64,
}

impl WriteRefused {
    pub(crate) fn new(state: ControlState, address: u64) -> Self {
        Self { state, address }
    }
}
