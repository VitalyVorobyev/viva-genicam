#![cfg_attr(docsrs, feature(doc_cfg))]
//! High level GenICam facade that re-exports the workspace crates and provides
//! convenience wrappers.
//!
//! ```rust,no_run
//! use viva_genicam::{gige, genapi, Camera, GenicamError};
//! use std::time::Duration;
//!
//! # struct DummyTransport;
//! # impl genapi::RegisterIo for DummyTransport {
//! #     fn read(&self, _addr: u64, len: usize) -> Result<Vec<u8>, genapi::GenApiError> {
//! #         Ok(vec![0; len])
//! #     }
//! #     fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), genapi::GenApiError> {
//! #         Ok(())
//! #     }
//! # }
//! # #[allow(dead_code)]
//! # fn load_nodemap() -> genapi::NodeMap {
//! #     unimplemented!("replace with GenApi XML parsing")
//! # }
//! # #[allow(dead_code)]
//! # async fn open_transport() -> Result<DummyTransport, GenicamError> {
//! #     Ok(DummyTransport)
//! # }
//! # #[allow(dead_code)]
//! # async fn run() -> Result<(), GenicamError> {
//! let timeout = Duration::from_millis(500);
//! let devices = gige::discover(timeout)
//!     .await
//!     .expect("discover cameras");
//! println!("found {} cameras", devices.len());
//! let mut camera = Camera::new(open_transport().await?, load_nodemap());
//! camera.set("ExposureTime", "5000")?;
//! # Ok(())
//! # }
//! ```
//!
//! ```rust,no_run
//! # async fn events_example(
//! #     mut camera: viva_genicam::Camera<viva_genicam::GigeRegisterIo>,
//! # ) -> Result<(), viva_genicam::GenicamError> {
//! use std::net::Ipv4Addr;
//! let ids = ["FrameStart", "ExposureEnd"];
//! let iface = Ipv4Addr::new(127, 0, 0, 1);
//! camera.configure_events(iface, 10020, &ids).await?;
//! let stream = camera.open_event_stream(iface, 10020).await?;
//! let event = stream.next().await?;
//! println!("event id=0x{:04X} payload={} bytes", event.id, event.data.len());
//! # Ok(())
//! # }
//! ```
//!
//! ```rust,no_run
//! # async fn action_example() -> Result<(), std::io::Error> {
//! use viva_genicam::gige::action::{send_action, ActionParams};
//! use std::net::SocketAddr;
//! let params = ActionParams {
//!     device_key: 0,
//!     group_key: 1,
//!     group_mask: 0xFFFF_FFFF,
//!     scheduled_time: None,
//! };
//! let dest: SocketAddr = "255.255.255.255:3956".parse().unwrap();
//! let summary = send_action(dest, &params, 200).await?;
//! println!("acks={}", summary.acks);
//! Ok(())
//! # }
//! ```

pub use viva_genapi as genapi;
pub use viva_gencp as gencp;
pub use viva_gige as gige;
pub use viva_pfnc as pfnc;
pub use viva_sfnc as sfnc;
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub use viva_u3v as u3v;

pub mod chunks;
pub mod events;
pub mod evs;
pub mod frame;
#[cfg(feature = "image-io")]
#[cfg_attr(docsrs, doc(cfg(feature = "image-io")))]
pub mod image_io;
pub mod report;
pub mod stream;
pub mod time;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::events::{
    bind_socket as bind_event_socket_internal,
    configure_message_channel_raw as configure_message_channel_fallback,
};
use crate::genapi::{GenApiError, Node, NodeMap, PortKind, RegisterIo, SkOutput};
use gige::GigeDevice;
use gige::gvcp::consts as gvcp_consts;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info, warn};

pub use chunks::{ChunkData, ChunkError, chunk_data};
pub use events::{Event, EventStream};
pub use evs::{EvsBlock, EvsFormat};
pub use frame::{Frame, FrameStatus};
pub use gige::action::{AckSummary, ActionParams};
pub use stream::{
    FrameStream, GenericStreamBlock, ResendPolicy, Stream, StreamBlock, StreamBuilder, StreamDest,
    StreamOptions, StreamStats,
};
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub use stream::{U3vFrameStream, U3vStreamBuilder};
pub use time::TimeSync;

/// Error type produced by the high level GenICam facade.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GenicamError {
    /// Wrapper around GenApi errors produced by the nodemap.
    #[error(transparent)]
    GenApi(#[from] GenApiError),
    /// Transport level failure while accessing registers.
    #[error("transport: {0}")]
    Transport(String),
    /// Parsing a user supplied value failed.
    #[error("parse error: {0}")]
    Parse(String),
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
        /// How long [`Camera::execute_and_wait`] polled.
        waited: Duration,
    },
    /// Nothing arrived within the time a blocking call was given.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
}

impl GenicamError {
    fn parse<S: Into<String>>(msg: S) -> Self {
        GenicamError::Parse(msg.into())
    }

    fn transport<S: Into<String>>(msg: S) -> Self {
        GenicamError::Transport(msg.into())
    }
}

/// How often [`Camera::execute_and_wait`] asks a command whether it has
/// finished.
///
/// GenICam leaves the interval to the application; a `<PollingTime>` in the
/// XML is not read yet (backlog GA-05). Ten milliseconds keeps a self-clearing
/// command's latency low at a cost of one register read per poll.
pub const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Camera facade combining a nodemap with a transport implementing [`RegisterIo`].
#[derive(Debug)]
pub struct Camera<T: RegisterIo> {
    transport: T,
    nodemap: NodeMap,
    time_sync: TimeSync,
}

impl<T: RegisterIo> Camera<T> {
    /// Create a new camera wrapper from a transport and a nodemap.
    pub fn new(transport: T, nodemap: NodeMap) -> Self {
        Self {
            transport,
            nodemap,
            time_sync: TimeSync::with_capacity(64),
        }
    }

    #[inline]
    fn with_map<R>(&mut self, f: impl FnOnce(&mut NodeMap, &T) -> R) -> R {
        let transport = &self.transport;
        let nodemap = &mut self.nodemap;
        f(nodemap, transport)
    }

    /// Return a reference to the underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Return a mutable reference to the underlying transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Access the nodemap metadata.
    pub fn nodemap(&self) -> &NodeMap {
        &self.nodemap
    }

    /// Mutable access to the nodemap.
    pub fn nodemap_mut(&mut self) -> &mut NodeMap {
        &mut self.nodemap
    }

    /// List available entries for an enumeration feature.
    pub fn enum_entries(&self, name: &str) -> Result<Vec<String>, GenicamError> {
        self.nodemap.enum_entries(name).map_err(Into::into)
    }

    /// Read everything a viewer needs about a feature at once: its typed
    /// value, what it permits now, whether it is implemented and available,
    /// its range, the enumeration entries it accepts, and its unit.
    ///
    /// See [`NodeMap::feature_state`](genapi::NodeMap::feature_state).
    pub fn feature_state(&self, name: &str) -> Result<genapi::FeatureState, GenicamError> {
        self.nodemap
            .feature_state(name, &self.transport)
            .map_err(Into::into)
    }

    /// Retrieve a feature value as a string using the nodemap type to format it.
    pub fn get(&self, name: &str) -> Result<String, GenicamError> {
        match self.nodemap.node(name) {
            Some(Node::Integer(_)) => {
                Ok(self.nodemap.get_integer(name, &self.transport)?.to_string())
            }
            Some(Node::Float(_)) => Ok(self.nodemap.get_float(name, &self.transport)?.to_string()),
            Some(Node::Enum(_)) => self
                .nodemap
                .get_enum(name, &self.transport)
                .map_err(Into::into),
            Some(Node::Boolean(_)) => Ok(self.nodemap.get_bool(name, &self.transport)?.to_string()),
            Some(Node::SwissKnife(sk)) => match sk.output {
                SkOutput::Float => Ok(self.nodemap.get_float(name, &self.transport)?.to_string()),
                SkOutput::Integer => {
                    Ok(self.nodemap.get_integer(name, &self.transport)?.to_string())
                }
            },
            Some(Node::Converter(conv)) => match conv.output {
                SkOutput::Float => Ok(self
                    .nodemap
                    .get_converter(name, &self.transport)?
                    .to_string()),
                SkOutput::Integer => {
                    Ok((self.nodemap.get_converter(name, &self.transport)? as i64).to_string())
                }
            },
            Some(Node::IntConverter(_)) => Ok(self
                .nodemap
                .get_int_converter(name, &self.transport)?
                .to_string()),
            Some(Node::String(_)) => self
                .nodemap
                .get_string(name, &self.transport)
                .map_err(Into::into),
            Some(Node::Command(_)) => {
                Err(GenicamError::GenApi(GenApiError::Type(name.to_string())))
            }
            // A `<Register>` is a byte array with no declared interpretation.
            // Rendering it into this `String`-typed API would mean inventing an
            // encoding — hex? base64? — that nothing asked for and no caller
            // could rely on. Raw access goes through
            // `nodemap().get_register(name, transport)`.
            Some(Node::Register(_)) => {
                Err(GenicamError::GenApi(GenApiError::Type(name.to_string())))
            }
            Some(Node::Category(_)) => Ok(String::new()),
            // `Node` is `#[non_exhaustive]`, so a node type added in
            // `viva-genapi` no longer breaks this crate's build. Name the kind
            // rather than returning a bare type error, so the gap is legible
            // from a log line instead of needing a debugger.
            Some(other) => Err(GenicamError::GenApi(GenApiError::Type(format!(
                "{name} is a {} node, which Camera::get cannot render as a string",
                other.kind_name()
            )))),
            None => Err(GenApiError::NodeNotFound(name.to_string()).into()),
        }
    }

    /// Set a feature value using a string representation.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), GenicamError> {
        match self.nodemap.node(name) {
            // A converter writes through its `<FormulaTo>`: `set_integer` and
            // `set_float` dispatch to it.
            Some(Node::Integer(_) | Node::IntConverter(_)) => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| GenicamError::parse(format!("invalid integer for {name}")))?;
                self.nodemap
                    .set_integer(name, parsed, &self.transport)
                    .map_err(Into::into)
            }
            Some(Node::Float(_) | Node::Converter(_)) => {
                let parsed: f64 = value
                    .parse()
                    .map_err(|_| GenicamError::parse(format!("invalid float for {name}")))?;
                self.nodemap
                    .set_float(name, parsed, &self.transport)
                    .map_err(Into::into)
            }
            Some(Node::Enum(_)) => self
                .nodemap
                .set_enum(name, value, &self.transport)
                .map_err(Into::into),
            Some(Node::Boolean(_)) => {
                let parsed = parse_bool(value).ok_or_else(|| {
                    GenicamError::parse(format!("invalid boolean for {name}: {value}"))
                })?;
                self.nodemap
                    .set_bool(name, parsed, &self.transport)
                    .map_err(Into::into)
            }
            Some(Node::SwissKnife(_)) => Err(GenApiError::Type(name.to_string()).into()),
            Some(Node::String(_)) => self
                .nodemap
                .set_string(name, value, &self.transport)
                .map_err(Into::into),
            Some(Node::Command(_)) => self
                .nodemap
                .exec_command(name, &self.transport)
                .map_err(Into::into),
            // See the matching arm in `get`: a byte array has no string form
            // this API could parse back. Use `nodemap().set_register(...)`.
            Some(Node::Register(_)) => Err(GenApiError::Type(name.to_string()).into()),
            Some(Node::Category(_)) => Err(GenApiError::Type(name.to_string()).into()),
            // See the matching arm in `get`.
            Some(other) => Err(GenicamError::GenApi(GenApiError::Type(format!(
                "{name} is a {} node, which Camera::set cannot write from a string",
                other.kind_name()
            )))),
            None => Err(GenApiError::NodeNotFound(name.to_string()).into()),
        }
    }

    /// Convenience wrapper for exposure time features expressed in microseconds.
    pub fn set_exposure_time_us(&mut self, value: f64) -> Result<(), GenicamError> {
        // Use SFNC name directly to avoid cross-crate constant lookup issues in docs
        self.set_float_feature("ExposureTime", value)
    }

    /// Convenience wrapper for gain features expressed in decibel.
    pub fn set_gain_db(&mut self, value: f64) -> Result<(), GenicamError> {
        self.set_float_feature("Gain", value)
    }

    fn set_float_feature(&mut self, name: &str, value: f64) -> Result<(), GenicamError> {
        match self.nodemap.node(name) {
            Some(Node::Float(_)) => self
                .nodemap
                .set_float(name, value, &self.transport)
                .map_err(Into::into),
            Some(_) => Err(GenApiError::Type(name.to_string()).into()),
            None => Err(GenApiError::NodeNotFound(name.to_string()).into()),
        }
    }

    /// Capture device/host timestamp pairs and fit a mapping model.
    pub async fn time_calibrate(
        &mut self,
        samples: usize,
        interval_ms: u64,
    ) -> Result<(), GenicamError> {
        if samples < 2 {
            return Err(GenicamError::transport(
                "time calibration requires at least two samples",
            ));
        }

        let cap = samples.max(self.time_sync.capacity());
        self.time_sync = TimeSync::with_capacity(cap);

        let latch_cmd = self.find_alias(viva_sfnc::TS_LATCH_CMDS);
        let value_node = self
            .find_alias(viva_sfnc::TS_VALUE_NODES)
            .ok_or_else(|| GenApiError::NodeNotFound("TimestampValue".into()))?;

        let mut freq_hz = if let Some(name) = self.find_alias(viva_sfnc::TS_FREQ_NODES) {
            match self.nodemap.get_integer(name, &self.transport) {
                Ok(value) if value > 0 => Some(value as f64),
                Ok(_) => None,
                Err(err) => {
                    debug!(node = name, error = %err, "failed to read timestamp frequency");
                    None
                }
            }
        } else {
            None
        };

        info!(samples, interval_ms, "starting time calibration");
        let mut first_sample: Option<(u64, Instant)> = None;
        let mut last_sample: Option<(u64, Instant)> = None;

        for idx in 0..samples {
            if let Some(cmd) = latch_cmd {
                self.nodemap
                    .exec_command(cmd, &self.transport)
                    .map_err(GenicamError::from)?;
            }

            // The latch changed the value behind the cache's back: unless the
            // XML links the two, the cached first reading came back every
            // time, no fit was possible, and calibration reported success on
            // default coefficients (found once a fit had to succeed, Codex
            // review of #185).
            self.nodemap.invalidate(value_node);
            let raw_ticks = self
                .nodemap
                .get_integer(value_node, &self.transport)
                .map_err(GenicamError::from)?;
            let dev_ticks = u64::try_from(raw_ticks).map_err(|_| {
                GenicamError::transport("timestamp value is negative; unsupported camera")
            })?;
            let host = Instant::now();
            self.time_sync.update(dev_ticks, host);
            if idx == 0 {
                first_sample = Some((dev_ticks, host));
            }
            last_sample = Some((dev_ticks, host));
            if let Some(origin) = self.time_sync.origin_instant() {
                let ns = host.duration_since(origin).as_nanos();
                debug!(
                    sample = idx,
                    ticks = dev_ticks,
                    host_ns = ns,
                    "timestamp sample"
                );
            } else {
                debug!(sample = idx, ticks = dev_ticks, "timestamp sample");
            }

            if interval_ms > 0 && idx + 1 < samples {
                sleep(Duration::from_millis(interval_ms)).await;
            }
        }

        if freq_hz.is_none()
            && let (Some((first_ticks, first_host)), Some((last_ticks, last_host))) =
                (first_sample, last_sample)
            && last_ticks > first_ticks
            && let Some(delta) = last_host.checked_duration_since(first_host)
        {
            let secs = delta.as_secs_f64();
            if secs > 0.0 {
                freq_hz = Some((last_ticks - first_ticks) as f64 / secs);
            }
        }

        let (a, b) = self
            .time_sync
            .fit(freq_hz)
            .ok_or_else(|| GenicamError::transport("insufficient samples for timestamp fit"))?;

        if let Some(freq) = freq_hz {
            info!(freq_hz = freq, a, b, "time calibration complete");
        } else {
            info!(a, b, "time calibration complete");
        }

        Ok(())
    }

    /// Map device tick counters to host time using the fitted model; `None`
    /// before [`time_calibrate`](Self::time_calibrate) has fitted one.
    pub fn map_dev_ts(&self, dev_ticks: u64) -> Option<SystemTime> {
        self.time_sync.to_host_time(dev_ticks)
    }

    /// Inspect the timestamp synchroniser state.
    pub fn time_sync(&self) -> &TimeSync {
        &self.time_sync
    }

    /// Reset the device timestamp counter when supported by the camera.
    pub fn time_reset(&mut self) -> Result<(), GenicamError> {
        if let Some(cmd) = self.find_alias(viva_sfnc::TS_RESET_CMDS) {
            self.nodemap
                .exec_command(cmd, &self.transport)
                .map_err(GenicamError::from)?;
            self.time_sync = TimeSync::with_capacity(self.time_sync.capacity());
            info!(command = cmd, "timestamp counter reset");
        }
        Ok(())
    }

    /// Execute a command feature by name.
    ///
    /// [`Camera::set`] already dispatches to this for `Node::Command`, but
    /// only from a `&str` value; callers that hold a command name directly
    /// would otherwise have to reach through [`Camera::nodemap_mut`] and
    /// [`Camera::transport`] at once, which the borrow checker rejects from
    /// outside the struct. Splitting the two fields here is the same thing
    /// [`Camera::acquisition_start`] does.
    pub fn execute_command(&mut self, name: &str) -> Result<(), GenicamError> {
        self.nodemap
            .exec_command(name, &self.transport)
            .map_err(Into::into)
    }

    /// Execute a command and wait until the camera reports it finished.
    ///
    /// GenICam Standard v2.1.1 §2.8.8: "the state of other nodes can depend on
    /// the completion of the Command therefore applications ... should always
    /// wait for the command completion by calling IsDone immediately after the
    /// Execute". This polls [`NodeMap::is_done`] every
    /// [`COMMAND_POLL_INTERVAL`] until it answers `true` or `timeout` passes.
    ///
    /// A command whose register path is write-only — most of them — is done
    /// as soon as the write is acknowledged, so this returns as quickly as
    /// [`Camera::execute_command`] does. A self-clearing command (its register
    /// reads back the command value until the device has finished) is waited
    /// on. Reads made afterwards see the state the command left.
    ///
    /// Returns [`GenicamError::CommandNotDone`] when the camera has not
    /// finished by `timeout`; the command was still executed.
    pub async fn execute_and_wait(
        &mut self,
        name: &str,
        timeout: Duration,
    ) -> Result<(), GenicamError> {
        self.execute_command(name)?;
        let started = Instant::now();
        loop {
            if self.nodemap.is_done(name, &self.transport)? {
                return Ok(());
            }
            let waited = started.elapsed();
            if waited >= timeout {
                return Err(GenicamError::CommandNotDone {
                    name: name.to_string(),
                    waited,
                });
            }
            sleep(COMMAND_POLL_INTERVAL.min(timeout - waited)).await;
        }
    }

    /// Whether the command `name` has finished: GenICam's `IsDone`.
    ///
    /// See [`NodeMap::is_done`]; [`Camera::execute_and_wait`] polls it.
    pub fn is_done(&self, name: &str) -> Result<bool, GenicamError> {
        self.nodemap
            .is_done(name, &self.transport)
            .map_err(Into::into)
    }

    /// Trigger acquisition start via the SFNC command feature.
    pub fn acquisition_start(&mut self) -> Result<(), GenicamError> {
        self.nodemap
            .exec_command("AcquisitionStart", &self.transport)
            .map_err(Into::into)
    }

    /// Trigger acquisition stop via the SFNC command feature.
    pub fn acquisition_stop(&mut self) -> Result<(), GenicamError> {
        self.nodemap
            .exec_command("AcquisitionStop", &self.transport)
            .map_err(Into::into)
    }

    /// Configure chunk mode and enable the requested selectors.
    pub fn configure_chunks(&mut self, cfg: &ChunkConfig) -> Result<(), GenicamError> {
        self.ensure_chunk_feature(viva_sfnc::CHUNK_MODE_ACTIVE)?;
        self.ensure_chunk_feature(viva_sfnc::CHUNK_SELECTOR)?;
        self.ensure_chunk_feature(viva_sfnc::CHUNK_ENABLE)?;

        // SAFE: split-borrow distinct fields of `self`
        self.with_map(|nm, tr| {
            nm.set_bool(viva_sfnc::CHUNK_MODE_ACTIVE, cfg.active, tr)?;
            for s in &cfg.selectors {
                nm.set_enum(viva_sfnc::CHUNK_SELECTOR, s, tr)?;
                nm.set_bool(viva_sfnc::CHUNK_ENABLE, cfg.active, tr)?;
            }
            Ok(())
        })
    }

    /// Read features from a frame's chunks: attach them to the nodemap's
    /// chunk ports, run `f`, and detach them again.
    ///
    /// Inside `f`, `cam.get("ChunkExposureTime")` reads the value this frame
    /// carried — from the chunk whose ID the camera's XML names, at the offset
    /// and in the byte order the XML declares (ADR-0026). Nothing is read from
    /// the device for it. A frame without chunks attaches none, and chunk
    /// features then fail with [`GenApiError::NotAttached`].
    ///
    /// ```rust,no_run
    /// # fn example(
    /// #     camera: &mut viva_genicam::Camera<viva_genicam::GigeRegisterIo>,
    /// #     frame: &viva_genicam::Frame,
    /// # ) -> Result<(), viva_genicam::GenicamError> {
    /// let exposure = camera.with_chunks(frame, |cam| cam.get("ChunkExposureTime"))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_chunks<R>(&mut self, frame: &Frame, f: impl FnOnce(&Self) -> R) -> R {
        let chunks = frame.chunks.clone().unwrap_or_default();
        self.nodemap.attach_chunks(&chunks);
        let result = f(self);
        self.nodemap.detach_chunks();
        result
    }

    /// Every feature `frame`'s chunks feed, with its value or the error that
    /// kept it from being read, sorted by name.
    ///
    /// The features are the ones the camera's XML lists in a category and
    /// derives from a chunk port whose chunk the frame carries — `ChunkTimestamp`,
    /// not the `ChunkTimestamp_Val` register behind it. Empty for a frame
    /// without chunks.
    pub fn chunk_features(&mut self, frame: &Frame) -> Vec<(String, Result<String, GenicamError>)> {
        let Some(chunks) = frame.chunks.as_ref() else {
            return Vec::new();
        };
        let ids: Vec<u64> = chunks.ids().collect();
        let names = self.features_on_ports(
            |kind| matches!(kind, PortKind::Chunk { chunk_id } if ids.contains(&chunk_id)),
        );
        self.with_chunks(frame, |cam| {
            names
                .into_iter()
                .map(|name| {
                    let value = cam.get(&name);
                    (name, value)
                })
                .collect()
        })
    }

    /// Read features from a received event: attach it to the ports whose
    /// `<EventID>` is the event's, run `f`, and detach events again.
    ///
    /// Inside `f`, `cam.get("EventExposureEndTimestamp")` reads the event's
    /// own bytes at the offset the camera's XML declares — 0x8 for the
    /// timestamp on every corpus document, which is where the event header has
    /// it.
    pub fn with_event<R>(&mut self, event: &Event, f: impl FnOnce(&Self) -> R) -> R {
        self.nodemap
            .attach_event(u64::from(event.id), event.raw.clone());
        let result = f(self);
        self.nodemap.detach_events();
        result
    }

    /// Every feature `event` feeds, with its value or the error that kept it
    /// from being read, sorted by name: the category features derived from a
    /// port whose `<EventID>` is the event's. Empty when the XML declares no
    /// port for it.
    pub fn event_features(&mut self, event: &Event) -> Vec<(String, Result<String, GenicamError>)> {
        let id = u64::from(event.id);
        let names = self.features_on_ports(
            |kind| matches!(kind, PortKind::Event { event_id } if event_id == id),
        );
        self.with_event(event, |cam| {
            names
                .into_iter()
                .map(|name| {
                    let value = cam.get(&name);
                    (name, value)
                })
                .collect()
        })
    }

    /// The features fed by the ports `which` selects, sorted, without
    /// repeats.
    fn features_on_ports(&self, which: impl Fn(PortKind) -> bool) -> Vec<String> {
        let mut names: Vec<String> = self
            .nodemap
            .ports()
            .into_iter()
            .filter(|(_, kind)| which(*kind))
            .flat_map(|(port, _)| self.nodemap.port_features(port))
            .map(str::to_string)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Configure the GVCP message channel and enable delivery of the requested events.
    pub async fn configure_events(
        &mut self,
        local_ip: Ipv4Addr,
        port: u16,
        enable_ids: &[&str],
    ) -> Result<(), GenicamError> {
        info!(%local_ip, port, "configuring GVCP events");
        // Pre-compute aliases before taking a mutable borrow of the nodemap
        let msg_sel = self.find_alias(viva_sfnc::MSG_SEL);
        let msg_ip = self.find_alias(viva_sfnc::MSG_IP);
        let msg_port = self.find_alias(viva_sfnc::MSG_PORT);
        let msg_en = self.find_alias(viva_sfnc::MSG_EN);

        let channel_configured = self.with_map(|nodemap, transport| {
            let mut ok = true;

            if let Some(selector) = msg_sel {
                match nodemap.enum_entries(selector) {
                    Ok(entries) => {
                        if let Some(entry) = entries.into_iter().next() {
                            if let Err(err) = nodemap.set_enum(selector, &entry, transport) {
                                warn!(node = selector, error = %err, "failed to set message selector");
                                ok = false;
                            }
                        } else {
                            warn!(node = selector, "message selector missing entries");
                            ok = false;
                        }
                    }
                    Err(err) => {
                        warn!(feature = selector, error = %err, "failed to query message selector");
                        ok = false;
                    }
                }
            } else {
                ok = false;
            }

            if let Some(node) = msg_ip {
                let value = u32::from(local_ip) as i64;
                if let Err(err) = nodemap.set_integer(node, value, transport) {
                    warn!(feature = node, error = %err, "failed to write message IP");
                    ok = false;
                }
            } else {
                ok = false;
            }

            if let Some(node) = msg_port {
                if let Err(err) = nodemap.set_integer(node, port as i64, transport) {
                    warn!(feature = node, error = %err, "failed to write message port");
                    ok = false;
                }
            } else {
                ok = false;
            }

            if let Some(node) = msg_en {
                if let Err(err) = nodemap.set_bool(node, true, transport) {
                    warn!(feature = node, error = %err, "failed to enable message channel");
                    ok = false;
                }
            } else {
                ok = false;
            }

            ok
        });

        if !channel_configured {
            configure_message_channel_fallback(&self.transport, local_ip, port)?;
        }

        let mut used_sfnc = self.nodemap.node(viva_sfnc::EVENT_SELECTOR).is_some()
            && self.nodemap.node(viva_sfnc::EVENT_NOTIFICATION).is_some();

        used_sfnc = self.with_map(|nodemap, transport| {
            if !used_sfnc {
                return false;
            }
            for &name in enable_ids {
                if let Err(err) = nodemap.set_enum(viva_sfnc::EVENT_SELECTOR, name, transport) {
                    warn!(event = name, error = %err, "failed to select event via SFNC");
                    return false;
                }
                if let Err(err) = nodemap.set_enum(
                    viva_sfnc::EVENT_NOTIFICATION,
                    viva_sfnc::EVENT_NOTIF_ON,
                    transport,
                ) {
                    warn!(event = name, error = %err, "failed to enable event via SFNC");
                    return false;
                }
            }
            true
        });

        if !used_sfnc && !enable_ids.is_empty() {
            // The message channel is addressable through bootstrap registers,
            // but which events a device emits is a GenApi decision with no
            // bootstrap equivalent. Say so rather than writing somewhere
            // hopeful.
            return Err(GenicamError::transport(format!(
                "cannot enable events {enable_ids:?}: camera exposes neither \
                 `{}` nor `{}`, and event selection has no bootstrap register",
                viva_sfnc::EVENT_SELECTOR,
                viva_sfnc::EVENT_NOTIFICATION,
            )));
        }

        Ok(())
    }

    /// Configure the stream channel for multicast delivery.
    pub fn configure_stream_multicast(
        &mut self,
        stream_idx: u32,
        group: Ipv4Addr,
        port: u16,
    ) -> Result<(), GenicamError> {
        if (group.octets()[0] & 0xF0) != 0xE0 {
            return Err(GenicamError::transport(
                "multicast group must be within 224.0.0.0/4",
            ));
        }
        info!(stream_idx, %group, port, "configuring multicast stream");

        // Precompute node names before taking &mut self.nodemap
        let dest_addr_node = self.find_alias(viva_sfnc::SCP_DEST_ADDR);
        let host_port_node = self.find_alias(viva_sfnc::SCP_HOST_PORT);
        let mcast_en_node = self.find_alias(viva_sfnc::MULTICAST_ENABLE);

        let mut used_sfnc = true;
        self.with_map(|nm, tr| {
            if nm.node(viva_sfnc::STREAM_CH_SELECTOR).is_some() {
                if let Err(err) =
                    nm.set_integer(viva_sfnc::STREAM_CH_SELECTOR, stream_idx as i64, tr)
                {
                    warn!(
                        channel = stream_idx,
                        error = %err,
                        "failed to select stream channel via SFNC"
                    );
                    used_sfnc = false;
                }
            } else {
                used_sfnc = false;
            }

            if let Some(node) = dest_addr_node {
                if let Err(err) = nm.set_integer(node, u32::from(group) as i64, tr) {
                    warn!(feature = node, error = %err, "failed to write multicast address");
                    used_sfnc = false;
                }
            } else {
                used_sfnc = false;
            }

            if let Some(node) = host_port_node {
                if let Err(err) = nm.set_integer(node, port as i64, tr) {
                    warn!(feature = node, error = %err, "failed to write multicast port");
                    used_sfnc = false;
                }
            } else {
                used_sfnc = false;
            }

            if let Some(node) = mcast_en_node {
                let _ = nm.set_bool(node, true, tr);
            }
        });

        if !used_sfnc {
            let base = gvcp_consts::STREAM_CHANNEL_BASE
                + stream_idx as u64 * gvcp_consts::STREAM_CHANNEL_STRIDE;
            let addr_reg = base + gvcp_consts::STREAM_DESTINATION_ADDRESS;
            self.transport
                .write(addr_reg, &group.octets())
                .map_err(|err| GenicamError::transport(format!("write multicast addr: {err}")))?;
            let port_reg = base + gvcp_consts::STREAM_DESTINATION_PORT;
            self.transport
                .write(port_reg, &port.to_be_bytes())
                .map_err(|err| GenicamError::transport(format!("write multicast port: {err}")))?;
            info!(
                stream_idx,
                %group,
                port,
                "configured multicast destination via raw registers"
            );
        } else {
            info!(
                stream_idx,
                %group,
                port,
                "configured multicast destination via SFNC"
            );
        }

        Ok(())
    }

    /// Open a GVCP event stream bound to the provided local endpoint.
    pub async fn open_event_stream(
        &self,
        local_ip: Ipv4Addr,
        port: u16,
    ) -> Result<EventStream, GenicamError> {
        let socket = bind_event_socket_internal(IpAddr::V4(local_ip), port).await?;
        let time_sync = if !self.time_sync.is_empty() {
            Some(Arc::new(self.time_sync.clone()))
        } else {
            None
        };
        Ok(EventStream::new(socket, time_sync))
    }

    fn ensure_chunk_feature(&self, name: &str) -> Result<(), GenicamError> {
        if self.nodemap.node(name).is_none() {
            return Err(GenicamError::MissingChunkFeature(name.to_string()));
        }
        Ok(())
    }

    fn find_alias(&self, names: &[&'static str]) -> Option<&'static str> {
        names
            .iter()
            .copied()
            .find(|name| self.nodemap.node(name).is_some())
    }
}

/// Configuration for enabling chunk data via SFNC features.
#[derive(Debug, Clone, Default)]
pub struct ChunkConfig {
    /// Names of chunk selectors that should be enabled on the device.
    pub selectors: Vec<String>,
    /// Whether chunk mode should be active after configuration.
    pub active: bool,
}

/// Blocking adapter turning an asynchronous [`GigeDevice`] into a [`RegisterIo`]
/// implementation.
///
/// The adapter uses [`tokio::runtime::Handle::block_on`] to synchronously wait
/// on GVCP register transactions.  When called from within a tokio runtime
/// context it automatically wraps the call in [`tokio::task::block_in_place`]
/// so the executor can keep making progress.  This makes it safe to call from
/// both async and plain synchronous contexts.
///
/// **Note:** `block_in_place` requires a multi-thread runtime.  Using a
/// `current_thread` runtime will still panic.
///
/// An access of exactly one aligned 32-bit register goes out as
/// READREG/WRITEREG, anything else as READMEM/WRITEMEM; see
/// [`GigeDevice::read_register_or_mem`](gige::GigeDevice::read_register_or_mem)
/// for the fallback a device that refuses the register commands gets.
///
/// # Control channel keepalive
///
/// Constructing the adapter spawns a background task that keeps the device's
/// control channel alive; see [`GigeRegisterIo::new`]. Holding a
/// `GigeRegisterIo` is therefore enough to keep control privilege — no caller
/// has to run a heartbeat of its own.
/// While register I/O holds the device mutex, the GVCP receive loop sends its
/// own probes on the same socket. Long ACK waits therefore do not starve the
/// idle task's keepalive; ordinary requests remain serialized.
pub struct GigeRegisterIo {
    handle: tokio::runtime::Handle,
    device: Arc<Mutex<GigeDevice>>,
    keepalive_stop: Option<tokio::sync::watch::Sender<bool>>,
    keepalive_task: Option<tokio::task::JoinHandle<()>>,
}

impl GigeRegisterIo {
    /// Create a new adapter using the provided runtime handle and device.
    ///
    /// This also spawns the control-channel keepalive on `handle`. A GigE Vision
    /// device revokes control privilege if it receives no GVCP command within
    /// `GevHeartbeatTimeout` (3 000 ms on both `viva-fake-gige` and the aravis
    /// fake camera, `arvfakecamera.c:1082`), and GVSP image traffic does not
    /// count — so a camera can sit at an interactive prompt, or stream frames for
    /// a minute, and then refuse the next write with `ACCESS_DENIED`.
    ///
    /// Nothing in an application's normal flow refreshes that timer, which is why
    /// three consumers of this crate had each grown their own copy of the loop —
    /// and why the Python bindings, which had none, hit the failure.
    ///
    /// The keepalive stops when the adapter is dropped, so privilege is
    /// maintained for exactly as long as the transport that needs it.
    /// Dropping the adapter cancels and drains any in-flight keepalive I/O.
    /// The runtime must remain alive until the adapter has been dropped. On a
    /// current-thread runtime the async task is aborted, since blocking on its
    /// completion would prevent that same runtime from making progress.
    pub fn new(handle: tokio::runtime::Handle, mut device: GigeDevice) -> Self {
        // A register operation holds the device mutex while waiting for its
        // reply. Let the socket owner send heartbeats during that wait; the
        // background task below covers idle periods and refines this period.
        device.set_inflight_keepalive_period(Some(keepalive_period(None)));
        let device = Arc::new(Mutex::new(device));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let task = spawn_keepalive(handle.clone(), Arc::downgrade(&device), stopped);
        Self {
            handle,
            device,
            keepalive_stop: Some(stop),
            keepalive_task: Some(task),
        }
    }

    /// Lock the underlying [`GigeDevice`] for direct async operations.
    ///
    /// This is intended for callers that need the raw device (e.g. stream
    /// channel configuration) while the `Camera` wrapper holds the transport.
    pub fn lock_device(&self) -> Result<MutexGuard<'_, GigeDevice>, GenicamError> {
        self.device
            .lock()
            .map_err(|_| GenicamError::transport("gige device mutex poisoned"))
    }

    fn lock(&self) -> Result<MutexGuard<'_, GigeDevice>, GenApiError> {
        self.device
            .lock()
            .map_err(|_| GenApiError::Io("gige device mutex poisoned".into()))
    }
}

impl Drop for GigeRegisterIo {
    fn drop(&mut self) {
        if let Some(stop) = self.keepalive_stop.take() {
            let _ = stop.send(true);
        }
        if let Some(task) = self.keepalive_task.take() {
            if self.handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
                // This task cannot progress while its runtime thread is in Drop.
                // Abort it and synchronize with the blocking operation instead.
                // Queued operations check cancellation before starting any I/O.
                task.abort();
                drop(self.device.lock());
                return;
            }
            // Aborting the async task would detach its spawn_blocking operation,
            // which can still use Tokio timers. Join both before runtime shutdown.
            let joined = match tokio::runtime::Handle::try_current() {
                Ok(current)
                    if current.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread =>
                {
                    // The adapter belongs to a different, multi-thread runtime.
                    // block_in_place is unavailable in the caller's runtime.
                    match std::thread::scope(|scope| {
                        scope.spawn(|| self.handle.block_on(task)).join()
                    }) {
                        Ok(joined) => joined,
                        Err(_) => {
                            warn!("keepalive shutdown thread panicked");
                            return;
                        }
                    }
                }
                Ok(_) => tokio::task::block_in_place(|| self.handle.block_on(task)),
                Err(_) => self.handle.block_on(task),
            };
            if let Err(error) = joined {
                warn!(%error, "control channel keepalive failed during shutdown");
            }
        }
    }
}

impl GigeRegisterIo {
    /// Run `fut` to completion on the adapter's runtime from synchronous code:
    /// inside a multi-thread runtime through `block_in_place`, elsewhere
    /// directly. The same rule every register access follows.
    pub(crate) fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.handle.block_on(fut))
        } else {
            self.handle.block_on(fut)
        }
    }
}

impl RegisterIo for GigeRegisterIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let mut device = self.lock()?;
        let fut = device.read_register_or_mem(addr, len);
        self.block_on(fut)
            .map_err(|err| GenApiError::Io(err.to_string()))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let mut device = self.lock()?;
        let fut = device.write_register_or_mem(addr, data);
        self.block_on(fut)
            .map_err(|err| GenApiError::Io(err.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Control channel keepalive
// ---------------------------------------------------------------------------

/// Heartbeat window assumed when the device does not report a usable one.
///
/// Both `viva-fake-gige` and the aravis fake camera report 3 000 ms, and it is
/// short enough to be a safe assumption for a device that tells us nothing.
const ASSUMED_HEARTBEAT_TIMEOUT_MS: u32 = 3_000;

/// How many keepalives we aim to fit inside one heartbeat window.
///
/// Four means three consecutive pings can be lost — to packet loss, to a long
/// `lock_device()` hold during stream setup, to a scheduling stall — and the
/// fourth still lands inside the window.
const KEEPALIVES_PER_WINDOW: u32 = 4;

/// Floor on the keepalive period, so a device reporting an implausibly small
/// timeout cannot turn the keepalive into a flood.
const MIN_KEEPALIVE_PERIOD: Duration = Duration::from_millis(100);

/// Ceiling on the keepalive period. A device may report a very long window; the
/// keepalive is also our liveness check, so it stays reasonably prompt.
const MAX_KEEPALIVE_PERIOD: Duration = Duration::from_secs(2);

/// Choose a keepalive period from the device's reported `GevHeartbeatTimeout`.
///
/// `None` means the register could not be read; `Some(0)` is a device declining
/// to say. Both fall back to [`ASSUMED_HEARTBEAT_TIMEOUT_MS`] rather than to a
/// period derived from a value we do not trust.
///
/// aravis instead pings on a fixed 1 s period
/// (`ARV_GV_DEVICE_HEARTBEAT_PERIOD_US`), which is fine for the 3 000 ms window
/// devices usually report but too slow for one that asks for less. Deriving the
/// period costs a single extra register read at connect.
fn keepalive_period(reported_timeout_ms: Option<u32>) -> Duration {
    let timeout_ms = match reported_timeout_ms {
        Some(ms) if ms > 0 => ms,
        _ => ASSUMED_HEARTBEAT_TIMEOUT_MS,
    };
    Duration::from_millis(u64::from(timeout_ms / KEEPALIVES_PER_WINDOW))
        .clamp(MIN_KEEPALIVE_PERIOD, MAX_KEEPALIVE_PERIOD)
}

/// Run one GVCP transaction against the shared device from the blocking pool.
///
/// Returns `None` once the transport has been dropped — the keepalive's exit
/// signal. `spawn_blocking` rather than `block_in_place` because the device
/// mutex can be held for the whole of stream negotiation, and waiting for it on
/// the blocking pool costs the runtime a pool thread rather than a worker.
async fn on_blocking_pool<R, F>(
    handle: &tokio::runtime::Handle,
    device: &std::sync::Weak<Mutex<GigeDevice>>,
    stopped: &mut tokio::sync::watch::Receiver<bool>,
    op: F,
) -> Option<Result<R, String>>
where
    F: FnOnce(
            &tokio::runtime::Handle,
            &mut GigeDevice,
            &mut tokio::sync::watch::Receiver<bool>,
        ) -> Result<R, String>
        + Send
        + 'static,
    R: Send + 'static,
{
    if *stopped.borrow() {
        return None;
    }
    let device = device.upgrade()?;
    let handle = handle.clone();
    let mut blocking_stopped = stopped.clone();
    let mut task = tokio::task::spawn_blocking(move || match device.lock() {
        Ok(mut guard) => {
            if *blocking_stopped.borrow() {
                return Err("keepalive stopped".to_string());
            }
            op(&handle, &mut guard, &mut blocking_stopped)
        }
        Err(_) => Err("gige device mutex poisoned".to_string()),
    });
    let joined = tokio::select! {
        joined = &mut task => joined,
        _ = stopped.changed() => {
            // The blocking thread cancels its I/O future; join it so no timer
            // registration can race the runtime's shutdown.
            let _ = task.await;
            return None;
        }
    };
    if *stopped.borrow() {
        return None;
    }
    Some(joined.unwrap_or_else(|err| Err(err.to_string())))
}

/// Keep the device's control channel alive for as long as the transport exists.
///
/// The adapter signals cancellation and drains blocking I/O on drop, before
/// the owning runtime shuts down.
///
/// It contends only for the device mutex — never for whatever lock the
/// application puts around its [`Camera`] — which is what lets it run without
/// the pause/resume dance every app-layer copy of this loop needed.
fn spawn_keepalive(
    handle: tokio::runtime::Handle,
    device: std::sync::Weak<Mutex<GigeDevice>>,
    mut stopped: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    handle.clone().spawn(async move {
        let reported = on_blocking_pool(&handle, &device, &mut stopped, |handle, dev, stopped| {
            let reported = handle.block_on(async {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => Err("keepalive stopped".to_string()),
                    result = dev.heartbeat_timeout_ms() => result.map_err(|err| err.to_string()),
                }
            });
            if let Ok(timeout_ms) = reported {
                dev.set_inflight_keepalive_period(
                    (timeout_ms != 0).then(|| keepalive_period(Some(timeout_ms))),
                );
            }
            reported
        })
        .await;

        let period = match reported {
            // Transport already gone; nothing to keep alive.
            None => return,
            Some(Ok(timeout_ms)) => {
                debug!(timeout_ms, "device reported GevHeartbeatTimeout");
                keepalive_period(Some(timeout_ms))
            }
            Some(Err(error)) => {
                warn!(
                    %error,
                    assumed_timeout_ms = ASSUMED_HEARTBEAT_TIMEOUT_MS,
                    "could not read GevHeartbeatTimeout; assuming a short window"
                );
                keepalive_period(None)
            }
        };
        debug!(?period, "control channel keepalive started");

        let mut ticker = tokio::time::interval(period);
        // After a long device-mutex hold (stream negotiation) one fresh ping is
        // enough; do not burst-fire the ticks that elapsed meanwhile.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` fires immediately, and the timeout read above has just
        // refreshed the timer.
        ticker.tick().await;

        let mut consecutive_failures: u32 = 0;
        let mut privilege_seen = false;
        loop {
            tokio::select! {
                _ = stopped.changed() => return,
                _ = ticker.tick() => {}
            }
            let pinged = on_blocking_pool(&handle, &device, &mut stopped, |handle, dev, stopped| {
                handle.block_on(async {
                    tokio::select! {
                        biased;
                        _ = stopped.changed() => Err("keepalive stopped".to_string()),
                        result = dev.ping_control_channel() => result.map_err(|err| err.to_string()),
                    }
                })
            })
            .await;
            match pinged {
                None => {
                    debug!("control channel keepalive stopped: transport dropped");
                    return;
                }
                Some(Ok(true)) => {
                    if consecutive_failures > 0 {
                        info!(consecutive_failures, "control channel keepalive recovered");
                    }
                    consecutive_failures = 0;
                    privilege_seen = true;
                }
                Some(Ok(false)) if !privilege_seen => {
                    // The caller never claimed control. A read-only session is
                    // legitimate — `viva-camctl xml` is one — and there is no
                    // privilege to keep alive, so stop rather than warn about it
                    // every period.
                    debug!("no control privilege held; keepalive not needed");
                    return;
                }
                Some(Ok(false)) => {
                    consecutive_failures += 1;
                    warn!(
                        consecutive_failures,
                        "control channel privilege lost; either another application took \
                         control or an earlier keepalive did not reach the device"
                    );
                }
                Some(Err(error)) => {
                    consecutive_failures += 1;
                    warn!(%error, consecutive_failures, "control channel keepalive failed");
                }
            }
        }
    })
}

/// Connect to a GigE Vision camera and return a fully configured [`Camera`].
///
/// This convenience function handles all connection boilerplate:
/// 1. Opens a GVCP control connection to the device
/// 2. Fetches and parses the GenApi XML from the camera
/// 3. Builds the nodemap
/// 4. Creates the transport adapter
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use viva_genicam::{gige, connect_gige};
///
/// let devices = gige::discover(Duration::from_millis(500)).await?;
/// let device = devices.into_iter().next().expect("no camera found");
/// let mut camera = connect_gige(&device).await?;
/// camera.set("ExposureTime", "5000")?;
/// ```
pub async fn connect_gige(
    device: &gige::DeviceInfo,
) -> Result<Camera<GigeRegisterIo>, GenicamError> {
    let (camera, _xml) = connect_gige_with_xml(device).await?;
    Ok(camera)
}

/// Connect to a GigE Vision camera and return both a [`Camera`] and the raw
/// GenICam XML string fetched from the device.
///
/// This is useful when the caller needs the XML for purposes beyond node
/// evaluation (e.g. forwarding it over a network API).
pub async fn connect_gige_with_xml(
    device: &gige::DeviceInfo,
) -> Result<(Camera<GigeRegisterIo>, String), GenicamError> {
    use std::net::{IpAddr, SocketAddr};
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), gige::GVCP_PORT);
    info!(%control_addr, "connecting to GigE Vision camera");

    let mut device = gige::GigeDevice::open(control_addr)
        .await
        .map_err(|e| GenicamError::transport(e.to_string()))?;

    // Claim control privilege (required before configuration and streaming).
    device
        .claim_control()
        .await
        .map_err(|e| GenicamError::transport(e.to_string()))?;

    let control = Arc::new(AsyncMutex::new(device));

    // Fetch and parse the GenApi XML.
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let control = control.clone();
        move |address, length| {
            let control = control.clone();
            async move {
                let mut dev = control.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(|err| viva_genapi_xml::XmlError::Transport(err.to_string()))
            }
        }
    })
    .await
    .map_err(|e| GenicamError::transport(e.to_string()))?;

    let nodemap = build_nodemap(&xml)?;

    // Extract the device and create the blocking adapter.
    let handle = tokio::runtime::Handle::current();
    let control_device = Arc::try_unwrap(control)
        .map_err(|_| GenicamError::transport("control connection still in use"))?
        .into_inner();
    let transport = GigeRegisterIo::new(handle, control_device);

    info!("GigE camera connected successfully");
    Ok((Camera::new(transport, nodemap), xml))
}

// ---------------------------------------------------------------------------
// USB3 Vision transport (behind `u3v` feature)
// ---------------------------------------------------------------------------

/// Blocking [`RegisterIo`] adapter wrapping a [`U3vDevice`](u3v::device::U3vDevice).
///
/// Generic over `T: UsbTransfer` so that real hardware (`RusbTransfer`) and
/// test doubles (`MockUsbTransfer`, `FakeU3vTransport`) all work through the
/// same code path. USB operations are inherently synchronous, so this adapter
/// simply forwards calls through a `Mutex` for thread safety.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vRegisterIo<T: u3v::usb::UsbTransfer + 'static> {
    /// Shared with a [`U3vFrameStream`](crate::U3vFrameStream), which clears
    /// StreamEnable through it when it stops.
    device: Arc<Mutex<u3v::device::U3vDevice<T>>>,
}

#[cfg(feature = "u3v")]
impl<T: u3v::usb::UsbTransfer + 'static> U3vRegisterIo<T> {
    /// Create a new adapter wrapping a [`U3vDevice`](u3v::device::U3vDevice).
    pub fn new(device: u3v::device::U3vDevice<T>) -> Self {
        Self {
            device: Arc::new(Mutex::new(device)),
        }
    }

    /// Lock the underlying device for direct access (e.g. stream configuration).
    pub fn lock_device(&self) -> Result<MutexGuard<'_, u3v::device::U3vDevice<T>>, GenicamError> {
        self.device
            .lock()
            .map_err(|_| GenicamError::transport("u3v device mutex poisoned"))
    }

    /// The device, shared: a frame stream holds it to stop the stream.
    pub(crate) fn shared_device(&self) -> Arc<Mutex<u3v::device::U3vDevice<T>>> {
        Arc::clone(&self.device)
    }

    fn lock(&self) -> Result<MutexGuard<'_, u3v::device::U3vDevice<T>>, GenApiError> {
        self.device
            .lock()
            .map_err(|_| GenApiError::Io("u3v device mutex poisoned".into()))
    }
}

#[cfg(feature = "u3v")]
impl<T: u3v::usb::UsbTransfer + 'static> RegisterIo for U3vRegisterIo<T> {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let mut device = self.lock()?;
        device
            .read_mem(addr, len)
            .map_err(|e| GenApiError::Io(e.to_string()))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let mut device = self.lock()?;
        device
            .write_mem(addr, data)
            .map_err(|e| GenApiError::Io(e.to_string()))
    }
}

/// Connect to a USB3 Vision camera and return a fully configured [`Camera`].
///
/// This convenience function handles all connection boilerplate:
/// 1. Opens the USB device and claims U3V interfaces
/// 2. Reads ABRM/SBRM bootstrap registers
/// 3. Fetches and parses the GenApi XML from the manifest table
/// 4. Builds the nodemap and creates the transport adapter
///
/// # Example
///
/// ```rust,ignore
/// use viva_genicam::{u3v, connect_u3v};
///
/// let devices = u3v::discovery::discover()?;
/// let device = devices.into_iter().next().expect("no U3V camera found");
/// let mut camera = connect_u3v(&device)?;
/// camera.set("ExposureTime", "5000")?;
/// ```
#[cfg(feature = "u3v-usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v-usb")))]
pub fn connect_u3v(
    device: &u3v::discovery::U3vDeviceInfo,
) -> Result<Camera<U3vRegisterIo<u3v::usb::RusbTransfer>>, GenicamError> {
    let (camera, _xml) = connect_u3v_with_xml(device)?;
    Ok(camera)
}

/// Connect to a USB3 Vision camera and return both a [`Camera`] and the raw
/// GenICam XML string fetched from the device.
#[cfg(feature = "u3v-usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v-usb")))]
pub fn connect_u3v_with_xml(
    device_info: &u3v::discovery::U3vDeviceInfo,
) -> Result<(Camera<U3vRegisterIo<u3v::usb::RusbTransfer>>, String), GenicamError> {
    info!(
        vendor_id = device_info.vendor_id,
        product_id = device_info.product_id,
        "connecting to USB3 Vision camera"
    );

    let mut device = u3v::device::U3vDevice::open_device(device_info)
        .map_err(|e| GenicamError::transport(e.to_string()))?;

    let xml = device
        .fetch_xml()
        .map_err(|e| GenicamError::transport(e.to_string()))?;

    let nodemap = build_nodemap(&xml)?;
    let transport = U3vRegisterIo::new(device);

    info!("USB3 Vision camera connected successfully");
    Ok((Camera::new(transport, nodemap), xml))
}

/// Create a [`Camera`] from an already-opened [`U3vDevice`](u3v::device::U3vDevice)
/// with any [`UsbTransfer`](u3v::usb::UsbTransfer) backend.
///
/// This is the generic entry point for testing with fake or mock transports.
/// The device must have been opened and bootstrapped (ABRM/SBRM read)
/// before calling this function.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub fn open_u3v_device<T: u3v::usb::UsbTransfer + 'static>(
    mut device: u3v::device::U3vDevice<T>,
) -> Result<(Camera<U3vRegisterIo<T>>, String), GenicamError> {
    let xml = device
        .fetch_xml()
        .map_err(|e| GenicamError::transport(e.to_string()))?;
    let nodemap = build_nodemap(&xml)?;
    let transport = U3vRegisterIo::new(device);
    Ok((Camera::new(transport, nodemap), xml))
}

/// Parse a camera's GenApi XML and build its nodemap, reporting what we could
/// not represent.
///
/// Both layers isolate per-node failures, so a camera opens with a missing
/// feature rather than not at all. That only helps if the loss is visible:
/// every dropped node is logged here, and a summary line goes out at `warn`
/// so a user hitting an unsupported construct has something to report.
fn build_nodemap(xml: &str) -> Result<NodeMap, GenicamError> {
    let model =
        viva_genapi_xml::parse(xml).map_err(|e| GenicamError::parse(format!("GenApi XML: {e}")))?;
    // The nodemap absorbs the XML layer's losses, so one list covers both.
    let nodemap = NodeMap::try_from_xml(model)
        .map_err(|e| GenicamError::parse(format!("GenApi model: {e}")))?;
    for skipped in nodemap.skipped() {
        debug!(
            kind = %skipped.tag,
            node = skipped.name.as_deref().unwrap_or("<unnamed>"),
            error = %skipped.error,
            "GenApi node unavailable"
        );
    }

    if !nodemap.skipped().is_empty() {
        warn!(
            unavailable = nodemap.skipped().len(),
            features = nodemap.node_names().count(),
            "some camera features are unavailable; run `viva-camctl report` for the list \
             and please report them at https://github.com/VitalyVorobyev/viva-genicam/issues"
        );
    }
    Ok(nodemap)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers in memory, for driving `Camera` without a transport.
    #[derive(Default)]
    struct MemoryIo(Mutex<std::collections::HashMap<u64, Vec<u8>>>);

    impl RegisterIo for MemoryIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            let regs = self.0.lock().unwrap();
            let data = regs.get(&addr).cloned().unwrap_or_else(|| vec![0; len]);
            Ok(data)
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            self.0.lock().unwrap().insert(addr, data.to_vec());
            Ok(())
        }
    }

    /// `Camera::set` on a feature that is itself a `<Converter>` writes through
    /// its `<FormulaTo>`, as `NodeMap::set_float` already does.
    #[test]
    fn set_writes_a_converter_through_its_formula() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <IntReg Name="FrameRateRaw">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Endianess>BigEndian</Endianess>
                </IntReg>
                <Converter Name="AcquisitionFrameRateAbs">
                    <FormulaTo>FROM * 100</FormulaTo>
                    <FormulaFrom>TO / 100</FormulaFrom>
                    <pValue>FrameRateRaw</pValue>
                </Converter>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let mut camera = Camera::new(MemoryIo::default(), nodemap);

        camera
            .set("AcquisitionFrameRateAbs", "12.5")
            .expect("write the converter");

        assert_eq!(camera.get("FrameRateRaw").expect("read raw"), "1250");
        assert_eq!(
            camera.get("AcquisitionFrameRateAbs").expect("read back"),
            "12.5"
        );
    }

    /// A command register the device clears once it has finished: the first
    /// `busy_reads` reads after a write still return the command value.
    struct SelfClearingIo {
        addr: u64,
        busy_reads: usize,
        state: Mutex<(Vec<u8>, usize, usize)>,
    }

    impl SelfClearingIo {
        fn new(addr: u64, busy_reads: usize) -> Self {
            Self {
                addr,
                busy_reads,
                state: Mutex::new((vec![0; 4], 0, 0)),
            }
        }

        /// `(writes, reads)` seen at the command register.
        fn counts(&self) -> (usize, usize) {
            let state = self.state.lock().unwrap();
            (state.1, state.2)
        }
    }

    impl RegisterIo for SelfClearingIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            assert_eq!(addr, self.addr, "only the command register is read");
            let mut state = self.state.lock().unwrap();
            state.2 += 1;
            if state.2 > self.busy_reads {
                state.0 = vec![0; len];
            }
            Ok(state.0.clone())
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            assert_eq!(addr, self.addr);
            let mut state = self.state.lock().unwrap();
            state.0 = data.to_vec();
            state.1 += 1;
            Ok(())
        }
    }

    const COMMAND_XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Command Name="UserSetLoad">
                <pValue>UserSetLoadReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="UserSetLoadReg">
                <Address>0x4000</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <Command Name="TriggerSoftware">
                <pValue>TriggerSoftwareReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="TriggerSoftwareReg">
                <Address>0x4000</Address><Length>4</Length><AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    fn command_camera(io: SelfClearingIo) -> Camera<SelfClearingIo> {
        let model = viva_genapi_xml::parse(COMMAND_XML).expect("parse");
        Camera::new(io, NodeMap::try_from_xml(model).expect("build nodemap"))
    }

    /// GA-36: executing waits until the register no longer reads back the
    /// command value (GenICam Standard v2.1.1 §2.8.8, `IsDone`).
    #[tokio::test]
    async fn execute_and_wait_polls_until_the_command_clears() {
        let mut camera = command_camera(SelfClearingIo::new(0x4000, 3));
        camera
            .execute_and_wait("UserSetLoad", Duration::from_secs(5))
            .await
            .expect("finishes after three busy reads");
        assert_eq!(camera.transport().counts(), (1, 4));
    }

    /// GA-36: a command that never finishes is reported, not waited on
    /// forever and not reported as done.
    #[tokio::test]
    async fn execute_and_wait_gives_up_at_the_timeout() {
        let mut camera = command_camera(SelfClearingIo::new(0x4000, usize::MAX));
        let err = camera
            .execute_and_wait("UserSetLoad", Duration::from_millis(50))
            .await
            .expect_err("never clears");
        assert!(
            matches!(err, GenicamError::CommandNotDone { ref name, .. } if name == "UserSetLoad"),
            "{err:?}"
        );
        assert_eq!(camera.transport().counts().0, 1, "written once");
    }

    /// GA-36: a write-only command is done once written; its register is
    /// never read, which a device would refuse.
    #[tokio::test]
    async fn execute_and_wait_does_not_read_a_write_only_command() {
        let mut camera = command_camera(SelfClearingIo::new(0x4000, usize::MAX));
        camera
            .execute_and_wait("TriggerSoftware", Duration::from_secs(5))
            .await
            .expect("done at once");
        assert_eq!(camera.transport().counts(), (1, 0));
    }

    #[test]
    fn keepalive_period_fits_four_pings_in_the_reported_window() {
        assert_eq!(keepalive_period(Some(3_000)), Duration::from_millis(750));
        assert_eq!(keepalive_period(Some(1_000)), Duration::from_millis(250));
    }

    #[test]
    fn an_unusable_heartbeat_timeout_falls_back_to_a_short_window() {
        // A device that declines to report, and a register we could not read,
        // must not be believed as "0 ms" or "no timeout at all".
        let assumed = keepalive_period(Some(ASSUMED_HEARTBEAT_TIMEOUT_MS));
        assert_eq!(keepalive_period(None), assumed);
        assert_eq!(keepalive_period(Some(0)), assumed);
    }

    #[test]
    fn the_period_is_clamped_at_both_ends() {
        // 40 ms / 4 = 10 ms would be a flood; a 10-minute window would make the
        // keepalive useless as a liveness check.
        assert_eq!(keepalive_period(Some(40)), MIN_KEEPALIVE_PERIOD);
        assert_eq!(keepalive_period(Some(600_000)), MAX_KEEPALIVE_PERIOD);
    }
}
