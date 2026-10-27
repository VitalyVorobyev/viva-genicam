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
//! use viva_genicam::gige::{send_action, ActionParams};
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

pub mod blocking;
mod chunks;
mod error;
mod events;
mod evs;
mod frame;
#[cfg(feature = "image-io")]
#[cfg_attr(docsrs, doc(cfg(feature = "image-io")))]
pub mod image_io;
mod reopen;
pub mod report;
mod standard;
mod stream;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::events::{
    bind_socket as bind_event_socket_internal,
    configure_message_channel_raw as configure_message_channel_fallback,
};
use crate::genapi::{GenApiError, Node, NodeMap, PortKind, RegisterIo};
use gige::GigeDevice;
use gige::consts as gvcp_consts;
use tokio::time::sleep;
use tracing::{debug, info, warn};

pub use chunks::{ChunkData, ChunkError, chunk_data};
pub use error::{GenicamError, WriteRefused};
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub use events::U3vEventStream;
pub use events::{Event, EventStream};
pub use evs::{EvsBlock, EvsFormat};
pub use frame::{Frame, FrameStatus};
pub use gige::{AckSummary, ActionParams};
pub use reopen::{Reopen, Reopened, reopen_gige};
pub use standard::{ResolvedFeature, Roi, StandardFeature};
#[cfg(fuzzing)]
pub use stream::fuzz_assemble;
pub use stream::{
    FrameStream, GenericStreamBlock, ResendPolicy, Stream, StreamBlock, StreamBuilder, StreamDest,
    StreamOptions, StreamStats, StreamStatsAccumulator,
};
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub use stream::{U3vFrameStream, U3vStreamBuilder};
pub use viva_gige::TimeSync;
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub use viva_u3v::U3vStreamOptions;

/// How often [`Camera::execute_and_wait`] asks a command whether it has
/// finished, when the command declares no `<PollingTime>`.
///
/// A command's `<PollingTime>` is the camera's own answer (GenICam Standard
/// v2.1.1 §2.8.8) and is used when present. Without one, ten milliseconds
/// keeps a self-clearing command's latency low at a cost of one register read
/// per poll.
pub const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long the helpers that wait for a command — the user-set helpers,
/// among them — give it to finish, and what consumers of this crate wait by
/// default.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

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

    /// Forget `name`'s cached value, and those of the registers it is read
    /// through and of everything that depends on it, so the next read asks
    /// the camera: for a value the camera changed in a way its XML does not
    /// describe. See [`NodeMap::invalidate`].
    ///
    /// A value the XML declares `NoCache` needs none of this, and one with a
    /// `<PollingTime>` is read again once that has passed (ADR-0029).
    pub fn invalidate(&self, name: &str) {
        self.nodemap.invalidate(name);
    }

    /// Forget every cached value, after something that may have changed the
    /// camera wholesale. [`Camera::load_user_set`] does this itself.
    pub fn invalidate_all(&self) {
        self.nodemap.invalidate_all();
    }

    /// The nodemap's current generation, advanced by every write and every
    /// invalidation: where a viewer starts [`Camera::changed_since`] from.
    pub fn generation(&self) -> u64 {
        self.nodemap.generation()
    }

    /// The features — and the nodes behind them — that changed after
    /// generation `since`, in document order, and the generation now. A
    /// viewer re-reads the ones it shows. See [`NodeMap::changed_since`].
    pub fn changed_since(&self, since: u64) -> (Vec<String>, u64) {
        self.nodemap.changed_since(since)
    }

    /// Invalidate the values whose `<PollingTime>` has passed since they were
    /// read, and return their names with those of everything read through
    /// them: what a viewer should read again. Call it as often as the shortest
    /// polling time a viewer shows. See [`NodeMap::poll`].
    pub fn poll(&self) -> Vec<String> {
        self.nodemap.poll()
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

    /// Read an integer feature: an `<Integer>`, an `<IntConverter>`, an
    /// integer `<IntSwissKnife>`, or a `<Converter>` rounded to the nearest
    /// integer. See [`NodeMap::get_integer`].
    pub fn get_integer(&self, name: &str) -> Result<i64, GenicamError> {
        Ok(self.nodemap.get_integer(name, &self.transport)?)
    }

    /// Write an integer feature, within its bounds and increment.
    pub fn set_integer(&mut self, name: &str, value: i64) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_integer(name, value, &self.transport)?)
    }

    /// Read a float feature: a `<Float>`, a `<Converter>`, a float
    /// `<SwissKnife>`, or an `<IntConverter>`. An `<Integer>` is not a
    /// float: read it with [`Camera::get_integer`].
    pub fn get_float(&self, name: &str) -> Result<f64, GenicamError> {
        Ok(self.nodemap.get_float(name, &self.transport)?)
    }

    /// Write a float feature, within its bounds.
    pub fn set_float(&mut self, name: &str, value: f64) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_float(name, value, &self.transport)?)
    }

    /// Read a boolean feature.
    pub fn get_bool(&self, name: &str) -> Result<bool, GenicamError> {
        Ok(self.nodemap.get_bool(name, &self.transport)?)
    }

    /// Write a boolean feature.
    pub fn set_bool(&mut self, name: &str, value: bool) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_bool(name, value, &self.transport)?)
    }

    /// Read an enumeration: the name of its current entry.
    pub fn get_enum(&self, name: &str) -> Result<String, GenicamError> {
        Ok(self.nodemap.get_enum(name, &self.transport)?)
    }

    /// Write an enumeration by entry name.
    pub fn set_enum(&mut self, name: &str, entry: &str) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_enum(name, entry, &self.transport)?)
    }

    /// Read a `<StringReg>`.
    pub fn get_string(&self, name: &str) -> Result<String, GenicamError> {
        Ok(self.nodemap.get_string(name, &self.transport)?)
    }

    /// Write a `<StringReg>`, NUL-padded to its length.
    pub fn set_string(&mut self, name: &str, value: &str) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_string(name, value, &self.transport)?)
    }

    /// Read a `<Register>`'s bytes. See [`NodeMap::get_register`].
    pub fn get_register(&self, name: &str) -> Result<Vec<u8>, GenicamError> {
        Ok(self.nodemap.get_register(name, &self.transport)?)
    }

    /// Write a `<Register>`'s bytes, exactly its length. See
    /// [`NodeMap::set_register`].
    pub fn set_register(&mut self, name: &str, data: &[u8]) -> Result<(), GenicamError> {
        Ok(self.nodemap.set_register(name, data, &self.transport)?)
    }

    /// Read a feature as text, formatted by its type: for a program that
    /// does not know the type in advance, such as a command line. The typed
    /// accessors ([`Camera::get_integer`] and the like) read without the
    /// round trip. See [`NodeMap::get_text`](genapi::NodeMap::get_text).
    ///
    /// A category has no value and a command none to read: both are errors.
    pub fn get(&self, name: &str) -> Result<String, GenicamError> {
        if let Some(Node::Command(_)) = self.nodemap.node(name) {
            return Err(GenApiError::Type {
                name: name.to_string(),
                kind: "Command",
                expected: "a feature with a value (run a command with execute_command or \
                           execute_and_wait)",
            }
            .into());
        }
        Ok(self.nodemap.get_text(name, &self.transport)?)
    }

    /// Write a feature from text, parsed by its type: the counterpart of
    /// [`Camera::get`]. Text that does not parse as the feature's type is
    /// [`GenApiError::InvalidText`] (in [`GenicamError::GenApi`]), and nothing
    /// is written.
    ///
    /// A command is not set but executed, with [`Camera::execute_command`] or
    /// [`Camera::execute_and_wait`]: setting one is an error, and nothing is
    /// written. So is setting a category.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), GenicamError> {
        if let Some(Node::Command(_)) = self.nodemap.node(name) {
            return Err(GenApiError::Type {
                name: name.to_string(),
                kind: "Command",
                expected: "a feature that is set (run a command with execute_command or \
                           execute_and_wait)",
            }
            .into());
        }
        Ok(self.nodemap.set_text(name, value, &self.transport)?)
    }

    /// Save the camera's configuration — every feature its XML marks
    /// `<Streamable>` — to `out`, in the GenApi feature-file layout
    /// (ADR-0028). A feature behind a selector is saved under each of the
    /// selector's entries, and the selector is put back; behind a selector
    /// that cannot be set now — not implemented, not available, locked — it
    /// has one value, saved once.
    ///
    /// Returns what was saved, and in [`Configuration::omitted`] what was
    /// left out and why; the file carries the same as comments. Save while
    /// the camera is not acquiring: what `TLParamsLocked` locks is left out
    /// otherwise.
    ///
    /// [`Configuration::omitted`]: genapi::persistence::Configuration::omitted
    pub fn save_features(
        &mut self,
        out: &mut impl std::io::Write,
    ) -> Result<genapi::Configuration, GenicamError> {
        let configuration = self.nodemap.save_configuration(&self.transport);
        out.write_all(configuration.to_text().as_bytes())?;
        out.flush()?;
        Ok(configuration)
    }

    /// Restore a configuration [`Camera::save_features`] wrote, or another
    /// GenApi-based tool did in the same layout.
    ///
    /// Values are written in the order the camera's dependencies require, a
    /// write that fails is tried once more after the others, and the report
    /// lists every value that still could not be written, with the reason.
    /// A file saved from another camera model, or another major version of
    /// its XML, is loaded too, with a warning in the report. Only a file that
    /// cannot be read at all is an error.
    pub fn load_features(
        &mut self,
        input: &mut impl std::io::Read,
    ) -> Result<genapi::LoadReport, GenicamError> {
        let mut text = String::new();
        input.read_to_string(&mut text)?;
        let configuration = genapi::Configuration::parse(&text)?;
        Ok(self.load_configuration(&configuration))
    }

    /// Restore a configuration already read, such as the one
    /// [`Camera::save_features`] returned: [`Camera::load_features`] without
    /// the file.
    pub fn load_configuration(
        &mut self,
        configuration: &genapi::Configuration,
    ) -> genapi::LoadReport {
        self.nodemap
            .load_configuration(configuration, &self.transport)
    }

    /// Capture device/host timestamp pairs and fit a mapping model.
    pub async fn time_calibrate(
        &mut self,
        samples: usize,
        interval_ms: u64,
    ) -> Result<(), GenicamError> {
        if samples < 2 {
            return Err(GenicamError::TimeSync("it takes at least two samples"));
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
        } else if let Some(name) = self.find_alias(viva_sfnc::TS_INCREMENT_NODES) {
            // Nanoseconds a tick (GenCP's Timestamp Increment).
            match self.nodemap.get_integer(name, &self.transport) {
                Ok(ns) if ns > 0 => Some(1e9 / ns as f64),
                Ok(_) => None,
                Err(err) => {
                    debug!(node = name, error = %err, "failed to read timestamp increment");
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
            let dev_ticks = u64::try_from(raw_ticks)
                .map_err(|_| GenicamError::TimeSync("the camera's timestamp reads negative"))?;
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

        let (a, b) = self.time_sync.fit(freq_hz).ok_or(GenicamError::TimeSync(
            "the samples do not fit a clock: too few, or all at one instant",
        ))?;

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
    /// Execute". This polls [`NodeMap::is_done`] at the command's
    /// `<PollingTime>` — every [`COMMAND_POLL_INTERVAL`] when it declares
    /// none — until it answers `true` or `timeout` passes.
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
        while let Some(pause) = self.poll_done(name, started, timeout)? {
            sleep(pause).await;
        }
        Ok(())
    }

    /// [`Camera::execute_and_wait`] on the calling thread, for a program
    /// without an async runtime; it sleeps between polls.
    pub fn execute_and_wait_blocking(
        &mut self,
        name: &str,
        timeout: Duration,
    ) -> Result<(), GenicamError> {
        self.execute_command(name)?;
        let started = Instant::now();
        while let Some(pause) = self.poll_done(name, started, timeout)? {
            std::thread::sleep(pause);
        }
        Ok(())
    }

    /// One poll of a command executed at `started`: `None` once it is done,
    /// else how long to wait before asking again; `CommandNotDone` once
    /// `timeout` has passed.
    fn poll_done(
        &self,
        name: &str,
        started: Instant,
        timeout: Duration,
    ) -> Result<Option<Duration>, GenicamError> {
        if self.nodemap.is_done(name, &self.transport)? {
            return Ok(None);
        }
        let waited = started.elapsed();
        if waited >= timeout {
            return Err(GenicamError::CommandNotDone {
                name: name.to_string(),
                waited,
            });
        }
        let interval = self
            .nodemap
            .polling_time(name)
            .unwrap_or(COMMAND_POLL_INTERVAL);
        Ok(Some(interval.min(timeout - waited)))
    }

    /// Whether the command `name` has finished: GenICam's `IsDone`.
    ///
    /// See [`NodeMap::is_done`]; [`Camera::execute_and_wait`] polls it.
    pub fn is_done(&self, name: &str) -> Result<bool, GenicamError> {
        self.nodemap
            .is_done(name, &self.transport)
            .map_err(Into::into)
    }

    /// Start acquisition: set `TLParamsLocked`, then execute
    /// `AcquisitionStart`.
    ///
    /// `TLParamsLocked` is how the transport layer tells the camera's XML a
    /// stream is running, so that features which would change the payload —
    /// `Width`, `PixelFormat` — are locked until it stops (33 of the 38
    /// vendor-corpus documents declare it). A camera without it is started
    /// all the same; one that refuses it is started with a warning. If the
    /// start fails, the lock is cleared again.
    pub fn acquisition_start(&mut self) -> Result<(), GenicamError> {
        self.lock_transport_parameters(true);
        let started = self
            .nodemap
            .exec_command(viva_sfnc::ACQUISITION_START, &self.transport)
            .map_err(Into::into);
        if started.is_err() {
            self.lock_transport_parameters(false);
        }
        started
    }

    /// Stop acquisition: execute `AcquisitionStop`, then clear
    /// `TLParamsLocked` — cleared even when the stop fails, so the features
    /// it locked can be written again.
    pub fn acquisition_stop(&mut self) -> Result<(), GenicamError> {
        let stopped = self
            .nodemap
            .exec_command(viva_sfnc::ACQUISITION_STOP, &self.transport)
            .map_err(Into::into);
        self.lock_transport_parameters(false);
        stopped
    }

    /// Write `TLParamsLocked`, an `<Integer>` or a `<Boolean>`, if the
    /// camera has it; a refusal is logged, not returned.
    fn lock_transport_parameters(&mut self, locked: bool) {
        let name = viva_sfnc::TL_PARAMS_LOCKED;
        let written = match self.nodemap.node(name) {
            None => return,
            Some(Node::Boolean(_)) => self.nodemap.set_bool(name, locked, &self.transport),
            Some(_) => self
                .nodemap
                .set_integer(name, i64::from(locked), &self.transport),
        };
        if let Err(error) = written {
            warn!(%error, locked, "could not write TLParamsLocked");
        }
    }

    /// Configure chunk mode and enable the requested selectors.
    pub fn configure_chunks(&mut self, cfg: &ChunkConfig) -> Result<(), GenicamError> {
        self.ensure_chunk_feature(viva_sfnc::CHUNK_MODE_ACTIVE)?;
        // Only a selector asked for needs them: an Allied Vision Manta G-125B
        // declares `ChunkModeActive` alone.
        if !cfg.selectors.is_empty() {
            self.ensure_chunk_feature(viva_sfnc::CHUNK_SELECTOR)?;
            self.ensure_chunk_feature(viva_sfnc::CHUNK_ENABLE)?;
        }

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

    /// Turn on notification of each event named, through SFNC's
    /// `EventSelector` and `EventNotification`.
    ///
    /// Which events a device emits is a GenApi decision with no bootstrap
    /// equivalent: without those features, naming an event is
    /// [`GenicamError::NoEventSelection`] rather than a write somewhere
    /// hopeful.
    pub(crate) fn enable_events(&mut self, enable_ids: &[&str]) -> Result<(), GenicamError> {
        let sfnc = self.nodemap.node(viva_sfnc::EVENT_SELECTOR).is_some()
            && self.nodemap.node(viva_sfnc::EVENT_NOTIFICATION).is_some();
        let enabled = sfnc
            && self.with_map(|nodemap, transport| {
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
        if !enabled && !enable_ids.is_empty() {
            return Err(GenicamError::NoEventSelection {
                events: enable_ids.iter().map(ToString::to_string).collect(),
            });
        }
        Ok(())
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

/// What only a GigE Vision camera has: a GVCP message channel for events
/// and a stream channel that can go to a multicast group. Streams and
/// control privilege are here too ([`Camera::open_stream`],
/// [`GigeRegisterIo::control_state`]). A USB3 Vision camera has none of
/// them, so none of this compiles against one.
impl Camera<GigeRegisterIo> {
    /// Configure the GVCP message channel and enable delivery of the requested events.
    ///
    /// GigE Vision only:
    ///
    /// ```compile_fail
    /// # async fn f(mut camera: viva_genicam::Camera<viva_genicam::genapi::NullIo>) {
    /// let _ = camera.configure_events(std::net::Ipv4Addr::LOCALHOST, 10020, &[]).await;
    /// # }
    /// ```
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

        self.enable_events(enable_ids)
    }

    /// Configure the stream channel for multicast delivery: point `GevSCDA`
    /// at `group` and `GevSCPHostPort` at `port`.
    ///
    /// A multicast destination address is all it takes; SFNC has no
    /// separate enable. Without the SFNC nodes the bootstrap registers are
    /// written directly. [`Camera::open_stream`] with
    /// [`StreamOptions::multicast`] does this and starts receiving too.
    ///
    /// GigE Vision only:
    ///
    /// ```compile_fail
    /// # fn f(mut camera: viva_genicam::Camera<viva_genicam::genapi::NullIo>) {
    /// let _ = camera.configure_stream_multicast(0, std::net::Ipv4Addr::new(239, 0, 0, 1), 5000);
    /// # }
    /// ```
    pub fn configure_stream_multicast(
        &mut self,
        stream_idx: u32,
        group: Ipv4Addr,
        port: u16,
    ) -> Result<(), GenicamError> {
        configure_multicast(self, stream_idx, group, port)
    }

    /// Open a GVCP event stream bound to the provided local endpoint.
    ///
    /// GigE Vision only:
    ///
    /// ```compile_fail
    /// # async fn f(camera: viva_genicam::Camera<viva_genicam::genapi::NullIo>) {
    /// let _ = camera.open_event_stream(std::net::Ipv4Addr::LOCALHOST, 10020).await;
    /// # }
    /// ```
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
}

/// [`Camera::configure_stream_multicast`], over any register transport so
/// the register writes can be checked in memory.
fn configure_multicast<T: RegisterIo>(
    camera: &mut Camera<T>,
    stream_idx: u32,
    group: Ipv4Addr,
    port: u16,
) -> Result<(), GenicamError> {
    if !group.is_multicast() {
        return Err(GenicamError::NotMulticast(std::net::SocketAddrV4::new(
            group, port,
        )));
    }
    info!(stream_idx, %group, port, "configuring multicast stream");

    // Precompute node names before taking &mut camera.nodemap
    let dest_addr_node = camera.find_alias(viva_sfnc::SCP_DEST_ADDR);
    let host_port_node = camera.find_alias(viva_sfnc::SCP_HOST_PORT);

    let mut used_sfnc = true;
    camera.with_map(|nm, tr| {
        if nm.node(viva_sfnc::STREAM_CH_SELECTOR).is_some() {
            if let Err(err) = nm.set_integer(viva_sfnc::STREAM_CH_SELECTOR, stream_idx as i64, tr) {
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
    });

    if !used_sfnc {
        let base = gvcp_consts::STREAM_CHANNEL_BASE
            + stream_idx as u64 * gvcp_consts::STREAM_CHANNEL_STRIDE;
        let addr_reg = base + gvcp_consts::STREAM_DESTINATION_ADDRESS;
        camera
            .transport
            .write(addr_reg, &group.octets())
            .map_err(GenicamError::from)?;
        // GevSCPHostPort is a 32-bit register with the port in its low
        // half. The two port bytes alone land in the high half (SR-20),
        // the bug already fixed for GevMCP.
        let port_reg = base + gvcp_consts::STREAM_DESTINATION_PORT;
        camera
            .transport
            .write(port_reg, &u32::from(port).to_be_bytes())
            .map_err(GenicamError::from)?;
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

/// Configuration for enabling chunk data via SFNC features.
#[derive(Debug, Clone, Default)]
pub struct ChunkConfig {
    /// Names of chunk selectors that should be enabled on the device.
    pub selectors: Vec<String>,
    /// Whether chunk mode should be active after configuration.
    pub active: bool,
}

/// Where a GigE session stands with the camera's control channel privilege
/// (CCP). Only a session that holds it writes registers: in every other
/// state a write is refused before it reaches the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ControlState {
    /// This application holds control privilege and keeps it alive.
    Held,
    /// Opened read-only ([`GigeRegisterIo::monitor`]): nothing was claimed,
    /// nothing is written, no heartbeat is sent.
    Monitor,
    /// Privilege was lost: CCP reads without control bits — the heartbeat
    /// expired — or, on a camera that names its primary application, another
    /// application holds it. Checked by the keepalive, and after a write
    /// refused with `ACCESS_DENIED`, when a camera that does not name its
    /// primary application is also asked by claiming again
    /// ([`GigeDevice::holds_control`]). [`GigeRegisterIo::reclaim_control`]
    /// asks for it again.
    Lost,
    /// Given up with [`GigeRegisterIo::release_control`];
    /// [`GigeRegisterIo::reclaim_control`] takes it again, and so does
    /// [`Camera::open_stream`].
    Released,
}

/// How [`connect_gige_as`] opens a camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Access {
    /// Claim control privilege, keep it alive, release it when the camera is
    /// dropped. Fails while another application holds it.
    #[default]
    Control,
    /// Read only: claim nothing, write nothing, send no heartbeat. Works while
    /// another application controls the camera; every write is refused.
    Monitor,
    /// [`Access::Control`], claimed as exclusive access (CCP `0x1`): while it
    /// is held, the camera refuses other applications' reads as well as
    /// their writes, and a [`Access::Monitor`] cannot open it. An Allied
    /// Vision Manta G-125B granted it and answered another application's
    /// READREG and READMEM with `ACCESS_DENIED`, while still answering
    /// discovery. The session is [`ControlState::Held`], as with control.
    Exclusive,
}

/// Blocking adapter turning an asynchronous [`GigeDevice`] into a [`RegisterIo`]
/// implementation.
///
/// The adapter waits on GVCP register transactions from synchronous code,
/// whatever the caller runs on: a plain thread waits on the transport's
/// runtime directly; inside a multi-thread runtime the wait goes through
/// [`tokio::task::block_in_place`] so the executor keeps making progress;
/// on a current-thread runtime's own thread it moves to a short-lived thread,
/// since nothing may block that runtime in place.
///
/// [`connect_gige`] gives a camera opened from a current-thread runtime a
/// runtime of its own, as [`blocking::connect`] does for a program with none.
/// A camera built with [`GigeRegisterIo::new`] over a current-thread
/// runtime's handle can be used from that runtime's blocking pool, but not
/// from its own thread, which the socket needs free.
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
    /// Where the session stands with control privilege; the keepalive task
    /// reports a loss here.
    control: Arc<tokio::sync::watch::Sender<ControlState>>,
    keepalive: Mutex<Option<Keepalive>>,
    /// The runtime `handle` belongs to, when the transport owns it
    /// ([`connect_gige`] from a current-thread runtime, [`blocking::connect`]).
    /// Shut down after control is released, when the transport is dropped.
    runtime: Option<tokio::runtime::Runtime>,
}

/// The running keepalive task and the signal that stops it.
struct Keepalive {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

/// How long dropping a controlling adapter waits for the camera to
/// acknowledge the release of control privilege. A camera that does not
/// answer in time times the privilege out on its own.
const RELEASE_TIMEOUT: Duration = Duration::from_millis(500);

impl GigeRegisterIo {
    /// Create a new adapter using the provided runtime handle and a device
    /// that holds control privilege ([`GigeDevice::claim_control`]).
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
    /// The keepalive stops when the adapter is dropped, and control privilege
    /// is released then (CCP ← 0), so the next application — or the next run
    /// of this one — can claim it at once instead of waiting out the
    /// heartbeat timeout. Privilege lost meanwhile is reported through
    /// [`control_state`](Self::control_state).
    /// Dropping the adapter cancels and drains any in-flight keepalive I/O.
    /// The runtime must remain alive until the adapter has been dropped. On a
    /// current-thread runtime the async task is aborted, since blocking on its
    /// completion would prevent that same runtime from making progress.
    pub fn new(handle: tokio::runtime::Handle, mut device: GigeDevice) -> Self {
        // A register operation holds the device mutex while waiting for its
        // reply. Let the socket owner send heartbeats during that wait; the
        // background task below covers idle periods and refines this period.
        device.set_inflight_keepalive_period(Some(keepalive_period(None)));
        let adapter = Self::with_state(handle, device, ControlState::Held);
        adapter.start_keepalive();
        adapter
    }

    /// A read-only adapter: no keepalive, no in-flight probes, and every write
    /// refused before it reaches the wire. For a camera another application
    /// controls; see [`Access::Monitor`].
    pub fn monitor(handle: tokio::runtime::Handle, mut device: GigeDevice) -> Self {
        device.set_inflight_keepalive_period(None);
        Self::with_state(handle, device, ControlState::Monitor)
    }

    fn with_state(handle: tokio::runtime::Handle, device: GigeDevice, state: ControlState) -> Self {
        Self {
            handle,
            device: Arc::new(Mutex::new(device)),
            control: Arc::new(tokio::sync::watch::channel(state).0),
            keepalive: Mutex::new(None),
            runtime: None,
        }
    }

    /// Own the runtime this transport's handle belongs to, and shut it down
    /// when the transport is dropped.
    pub(crate) fn adopt_runtime(&mut self, runtime: tokio::runtime::Runtime) {
        self.runtime = Some(runtime);
    }

    /// Where this session stands with control privilege.
    pub fn control_state(&self) -> ControlState {
        *self.control.borrow()
    }

    /// Changes of [`control_state`](Self::control_state) as they happen: losing
    /// privilege is reported here, not only logged.
    pub fn watch_control(&self) -> tokio::sync::watch::Receiver<ControlState> {
        self.control.subscribe()
    }

    /// Check an owned control session now, using only reads of the control
    /// channel and, where supported, the primary application's address.
    ///
    /// A revoked privilege or `ACCESS_DENIED` while reading that channel marks
    /// the session [`ControlState::Lost`], notifies [`Self::watch_control`], and
    /// stops its keepalive. Dropping it then sends no control release to another
    /// application's camera. A network failure is returned with its cause and
    /// does not by itself establish lost ownership.
    ///
    /// This never claims control. A monitor, released session or already-lost
    /// session is returned unchanged, without I/O. On cameras that do not name
    /// their primary application, a successful controlled CCP read cannot
    /// distinguish the current holder from another controller (TC-38).
    pub fn refresh_control_state(&self) -> Result<ControlState, GenicamError> {
        let stopped = {
            let mut device = self.lock_device()?;
            if self.control_state() != ControlState::Held {
                return Ok(self.control_state());
            }
            let held = match self.block_on(device.ping_control_channel())? {
                Ok(held) => held,
                Err(gige::GigeError::Status(gige::GigeStatus::Common(
                    gencp::StatusCode::AccessDenied,
                ))) => false,
                Err(source) => return Err(error::gige("read the control channel")(source)),
            };
            if held {
                return Ok(self.control_state());
            }
            self.control.send_replace(ControlState::Lost);
            // Take only the old keepalive while its device is still locked.
            // A concurrent explicit reclaim may start a new one afterward.
            let stopped = self.keepalive.lock().ok().and_then(|mut k| k.take());
            if let Some(keepalive) = &stopped {
                let _ = keepalive.stop.send(true);
            }
            stopped
        };
        if let Some(stopped) = stopped {
            self.drain_keepalive(stopped);
        }
        Ok(self.control_state())
    }

    /// Ask for control privilege again — after it was lost or released, or
    /// to turn a monitor into a controller — and keep it alive.
    ///
    /// Fails, and changes nothing, while another application holds it.
    pub fn reclaim_control(&self) -> Result<(), GenicamError> {
        {
            let mut guard = self.lock_device()?;
            let device: &mut GigeDevice = &mut guard;
            let fut = async {
                device.claim_control().await?;
                if let Err(err) = device.enable_pending_ack().await {
                    warn!(error = %err, "could not enable PENDING_ACK");
                }
                Ok::<_, gige::GigeError>(())
            };
            self.block_on(fut)?.map_err(error::gige("claim control"))?;
            if device.inflight_keepalive_period().is_none() {
                device.set_inflight_keepalive_period(Some(keepalive_period(None)));
            }
        }
        self.control.send_replace(ControlState::Held);
        self.start_keepalive();
        info!("control channel privilege reclaimed");
        Ok(())
    }

    /// Give control privilege up now (CCP ← 0) and stop the keepalive,
    /// reporting whether the camera acknowledged. Dropping the adapter does
    /// the same, best effort. A release the camera does not take leaves the
    /// session as it was, keepalive running.
    ///
    /// A monitor holds nothing and writes nothing, so for one this does
    /// nothing.
    pub fn release_control(&self) -> Result<(), GenicamError> {
        if self.control_state() == ControlState::Monitor {
            return Ok(());
        }
        self.stop_keepalive();
        let result = {
            let mut device = self.lock_device()?;
            let fut = device.release_control();
            self.block_on(fut)
                .and_then(|released| released.map_err(error::gige("release control")))
        };
        match result {
            Ok(()) => {
                self.control.send_replace(ControlState::Released);
            }
            // The camera did not take it: whatever the session held, it still
            // holds, and the keepalive goes on keeping it — and on finding out
            // if it is gone.
            Err(_) if self.control_state() == ControlState::Held => self.start_keepalive(),
            Err(_) => {}
        }
        result
    }

    /// Lock the underlying [`GigeDevice`] for direct async operations.
    ///
    /// This is intended for callers that need the raw device (e.g. stream
    /// channel configuration) while the `Camera` wrapper holds the transport.
    pub fn lock_device(&self) -> Result<MutexGuard<'_, GigeDevice>, GenicamError> {
        self.device
            .lock()
            .map_err(|_| GenicamError::Panicked("a thread holding the camera's device lock"))
    }

    fn lock(&self) -> Result<MutexGuard<'_, GigeDevice>, GenApiError> {
        self.device.lock().map_err(|_| {
            GenApiError::io(GenicamError::Panicked(
                "a thread holding the camera's device lock",
            ))
        })
    }

    /// Spawn the keepalive unless one is running.
    fn start_keepalive(&self) {
        let Ok(mut keepalive) = self.keepalive.lock() else {
            return;
        };
        if keepalive.as_ref().is_some_and(|k| !k.task.is_finished()) {
            return;
        }
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let task = spawn_keepalive(
            self.handle.clone(),
            Arc::downgrade(&self.device),
            stopped,
            self.control.clone(),
        );
        *keepalive = Some(Keepalive { stop, task });
    }

    /// Stop the keepalive and wait for its I/O to drain.
    fn stop_keepalive(&self) {
        let Some(keepalive) = self.keepalive.lock().ok().and_then(|mut k| k.take()) else {
            return;
        };
        self.drain_keepalive(keepalive);
    }

    fn drain_keepalive(&self, Keepalive { stop, task }: Keepalive) {
        let _ = stop.send(true);
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
        match self.block_on_from_anywhere(task) {
            Some(Err(error)) => warn!(%error, "control channel keepalive failed during shutdown"),
            Some(Ok(())) => {}
            None => warn!("keepalive shutdown thread panicked"),
        }
    }

    /// [`Self::block_on`], for shutdown: `None` where it cannot run.
    fn block_on_from_anywhere<F>(&self, fut: F) -> Option<F::Output>
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        self.block_on(fut).ok()
    }

    /// Release control privilege on the way out, within [`RELEASE_TIMEOUT`].
    fn release_on_drop(&self) {
        let Ok(mut device) = self.device.lock() else {
            return;
        };
        if self.handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
            // Nothing can wait for the acknowledgement here: send and go.
            if let Err(error) = device.try_send_release() {
                debug!(%error, "could not send the control release");
            }
            return;
        }
        // Built inside the future: a timer needs the runtime's context.
        let device: &mut GigeDevice = &mut device;
        let release =
            async move { tokio::time::timeout(RELEASE_TIMEOUT, device.release_control()).await };
        match self.block_on_from_anywhere(release) {
            Some(Ok(Ok(()))) => debug!("control channel privilege released"),
            Some(Ok(Err(error))) => debug!(%error, "the camera did not take the control release"),
            Some(Err(_)) => debug!("no answer to the control release; the camera will time it out"),
            None => debug!("control release thread panicked"),
        }
    }

    /// Control was claimed through the raw device ([`Camera::open_stream`]
    /// does): a session that had lost or released it holds it again.
    pub(crate) fn note_claimed(&self) {
        let reclaimed = self.control.send_if_modified(|state| {
            let lapsed = matches!(state, ControlState::Lost | ControlState::Released);
            if lapsed {
                *state = ControlState::Held;
            }
            lapsed
        });
        if reclaimed {
            self.start_keepalive();
        }
    }

    /// A write the camera refused with `ACCESS_DENIED`: either the register
    /// refuses it, or this session no longer holds control. Ask the device
    /// which ([`GigeDevice::holds_control`]), and mark control lost only on
    /// its answer.
    fn check_refusal(&self, device: &mut GigeDevice, err: &gige::GigeError) {
        if !matches!(
            err,
            gige::GigeError::Status(gige::GigeStatus::Common(gencp::StatusCode::AccessDenied))
        ) || self.control_state() != ControlState::Held
        {
            return;
        }
        match self
            .block_on(device.holds_control())
            .and_then(|pinged| pinged.map_err(error::gige("read the control channel")))
        {
            Ok(false) => {
                self.control.send_replace(ControlState::Lost);
                warn!(
                    "the camera refused a write with ACCESS_DENIED: control privilege is lost \
                     (reclaim_control asks for it again)"
                );
            }
            Ok(true) => debug!("ACCESS_DENIED for this register; control is still held"),
            Err(error) => debug!(%error, "could not check control privilege after ACCESS_DENIED"),
        }
    }
}

impl Drop for GigeRegisterIo {
    fn drop(&mut self) {
        let held = self.control_state() == ControlState::Held;
        self.stop_keepalive();
        if held {
            self.release_on_drop();
        }
        // Whatever context this runs in, an async one included: shutting
        // down in the background is the only way that never blocks it.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl GigeRegisterIo {
    /// Run `fut` to completion on the adapter's runtime from synchronous
    /// code, in any context: on a plain thread directly; inside a
    /// multi-thread runtime through `block_in_place`; on a current-thread
    /// runtime's thread, which nothing may block in place, on a scoped
    /// thread of its own. The rule every register access follows.
    ///
    /// The one case that cannot work is a transport on a current-thread
    /// runtime used from that runtime's own thread: the runtime drives the
    /// socket only while that thread is free. From its blocking pool
    /// (`spawn_blocking`) the access works; from the thread itself Tokio
    /// refuses it with a panic, rather than a hang. [`connect_gige`] never
    /// builds such a transport.
    pub(crate) fn block_on<F>(&self, fut: F) -> Result<F::Output, GenicamError>
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        use tokio::runtime::RuntimeFlavor::CurrentThread;
        match tokio::runtime::Handle::try_current() {
            Ok(current)
                if current.runtime_flavor() == CurrentThread
                    && self.handle.runtime_flavor() != CurrentThread =>
            {
                std::thread::scope(|scope| scope.spawn(|| self.handle.block_on(fut)).join())
                    .map_err(|_| GenicamError::Panicked("the register access thread"))
            }
            Ok(_) => Ok(tokio::task::block_in_place(|| self.handle.block_on(fut))),
            Err(_) => Ok(self.handle.block_on(fut)),
        }
    }
}

/// A camera's own runtime, when no runtime the caller has can serve it: one
/// worker drives its socket, keepalive and timers.
pub(crate) fn camera_runtime() -> Result<tokio::runtime::Runtime, GenicamError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("viva-gige")
        .enable_all()
        .build()
        .map_err(error::network("start the camera's runtime"))
}

impl RegisterIo for GigeRegisterIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let mut device = self.lock()?;
        let fut = device.read_register_or_mem(addr, len);
        self.block_on(fut)
            .map_err(GenApiError::io)?
            .map_err(GenApiError::io)
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        // Only a session that holds control writes: the camera would refuse
        // it anyway, whether another application controls it or nobody does
        // (an Allied Vision Manta G-125B answers `ACCESS_DENIED` to both).
        match self.control_state() {
            ControlState::Held => {}
            state => return Err(GenApiError::io(WriteRefused::new(state, addr))),
        }
        let mut device = self.lock()?;
        let fut = device.write_register_or_mem(addr, data);
        self.block_on(fut).map_err(GenApiError::io)?.map_err(|err| {
            self.check_refusal(&mut device, &err);
            GenApiError::io(err)
        })
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
    control: Arc<tokio::sync::watch::Sender<ControlState>>,
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
            // 0 is a device declining to say, read as the assumed window by
            // the idle keepalive too: the in-flight probes follow the same
            // reading rather than switching off (TC-26).
            if let Ok(timeout_ms) = reported {
                dev.set_inflight_keepalive_period(Some(keepalive_period(Some(timeout_ms))));
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
                }
                Some(Ok(false)) => {
                    // CCP reads without control bits: the heartbeat expired, or
                    // the device released us. Nothing is left to keep alive
                    // until the application reclaims it.
                    control.send_if_modified(|state| {
                        let held = *state == ControlState::Held;
                        if held {
                            *state = ControlState::Lost;
                        }
                        held
                    });
                    warn!(
                        "control channel privilege lost: CCP reads without this application's \
                         bits, so a heartbeat expired or the camera gave the privilege up; \
                         reclaim_control asks for it again"
                    );
                    return;
                }
                Some(Err(error)) => {
                    consecutive_failures += 1;
                    warn!(%error, consecutive_failures, "control channel keepalive failed");
                }
            }
        }
    })
}

/// Connect to a GigE Vision camera with the given [`Access`], returning the
/// [`Camera`] and the GenICam XML it was built from.
///
/// [`Access::Control`] claims control privilege, switches `PENDING_ACK` on
/// where GVCP Capability says the camera has it, and keeps the privilege
/// alive until the camera is dropped, which releases it. It fails while
/// another application holds control. [`Access::Exclusive`] does the same
/// with exclusive access, which also keeps other applications from reading.
/// [`Access::Monitor`] writes nothing at all, so it works alongside an
/// application holding control, though not one holding exclusive access.
///
/// From a current-thread runtime the camera gets a runtime of its own — one
/// worker thread, shut down when the camera is dropped — since a
/// current-thread runtime cannot drive the camera's socket while its one
/// thread waits in a register read.
pub async fn connect_gige_as(
    device: &gige::DeviceInfo,
    access: Access,
) -> Result<(Camera<GigeRegisterIo>, String), GenicamError> {
    use tokio::runtime::RuntimeFlavor::CurrentThread;
    if tokio::runtime::Handle::current().runtime_flavor() != CurrentThread {
        return connect_here(device, access).await;
    }
    let runtime = BackgroundShutdown(Some(camera_runtime()?));
    let device = device.clone();
    let opened = match runtime
        .handle()
        .spawn(async move { connect_here(&device, access).await })
        .await
    {
        Ok(opened) => opened,
        Err(err) => Err(error::task("connect")(err)),
    };
    let (mut camera, xml) = opened?;
    camera.transport.adopt_runtime(runtime.into_inner());
    Ok((camera, xml))
}

/// A runtime that, dropped, shuts down in the background: dropping a runtime
/// blocks, which an async context forbids. The connect future that holds one
/// can be dropped anywhere it awaits — by a timeout, a `select!` — and a
/// plain `Runtime` dropped there panics (Codex on #199).
struct BackgroundShutdown(Option<tokio::runtime::Runtime>);

impl BackgroundShutdown {
    fn handle(&self) -> &tokio::runtime::Handle {
        self.0
            .as_ref()
            .expect("the runtime is present until into_inner")
            .handle()
    }

    fn into_inner(mut self) -> tokio::runtime::Runtime {
        self.0
            .take()
            .expect("the runtime is present until into_inner")
    }
}

impl Drop for BackgroundShutdown {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// A claimed control socket while XML is being fetched. A cancelled connect
/// must release it too, before a `GigeRegisterIo` exists to do that on drop.
struct OpeningGigeDevice {
    device: Option<tokio::sync::Mutex<GigeDevice>>,
    claimed: bool,
}

impl Drop for OpeningGigeDevice {
    fn drop(&mut self) {
        if self.claimed
            && let Some(device) = self.device.as_mut()
            && let Err(error) = device.get_mut().try_send_release()
        {
            debug!(%error, "could not send the cancelled connection's control release");
        }
    }
}

/// [`connect_gige_as`] on the runtime it is called from, which must be a
/// multi-thread one or one other threads drive.
pub(crate) async fn connect_here(
    device: &gige::DeviceInfo,
    access: Access,
) -> Result<(Camera<GigeRegisterIo>, String), GenicamError> {
    use std::net::{IpAddr, SocketAddr};
    use tokio::sync::Mutex as AsyncMutex;

    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), gige::GVCP_PORT);
    info!(%control_addr, ?access, "connecting to GigE Vision camera");

    let device = gige::GigeDevice::open(control_addr)
        .await
        .map_err(error::gige("open the control channel"))?;

    let mut opening = OpeningGigeDevice {
        device: Some(AsyncMutex::new(device)),
        claimed: false,
    };
    if access != Access::Monitor {
        let mut device = opening
            .device
            .as_ref()
            .expect("opening device")
            .lock()
            .await;
        // Claim control privilege (required before configuration and
        // streaming); every later claim, after a loss, asks for the same.
        device.set_exclusive_access(access == Access::Exclusive);
        // The camera can apply the claim before its ACK arrives. Arm the
        // release first so cancellation in that gap does not leave it held.
        opening.claimed = true;
        if let Err(source) = device.claim_control().await {
            // A device refusal acquired nothing; a missing/malformed ACK
            // may hide a successful claim and still needs a release.
            opening.claimed = !matches!(source, gige::GigeError::Status(_));
            return Err(error::gige("claim control")(source));
        }
        match device.enable_pending_ack().await {
            Ok(enabled) => debug!(enabled, "PENDING_ACK"),
            Err(err) => {
                warn!(error = %err, "could not enable PENDING_ACK; slow commands will be retried")
            }
        }
    }

    let control = opening.device.as_ref().expect("opening device");

    // Fetch and parse the GenApi XML.
    let fetched = viva_genapi_xml::fetch_and_load_xml(|address, length| {
        let control = &control;
        async move {
            let mut dev = control.lock().await;
            dev.read_mem(address, length)
                .await
                .map_err(viva_genapi_xml::XmlError::transport)
        }
    })
    .await;

    // Extract the device and create the blocking adapter. It owns the release
    // of control from here, so a failure below still gives control back.
    let handle = tokio::runtime::Handle::current();
    let control_device = opening.device.take().expect("opening device").into_inner();
    opening.claimed = false;
    let transport = match access {
        Access::Control | Access::Exclusive => GigeRegisterIo::new(handle, control_device),
        Access::Monitor => GigeRegisterIo::monitor(handle, control_device),
    };
    let xml = fetched?;
    let nodemap = build_nodemap(&xml)?;

    info!("GigE camera connected successfully");
    Ok((Camera::new(transport, nodemap), xml))
}

/// Connect to a GigE Vision camera and return a fully configured [`Camera`].
///
/// This convenience function handles all connection boilerplate:
/// 1. Opens a GVCP control connection to the device and claims control
/// 2. Fetches and parses the GenApi XML from the camera
/// 3. Builds the nodemap
/// 4. Creates the transport adapter, which releases control when dropped
///
/// [`connect_gige_as`] opens a read-only monitor instead.
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
/// evaluation (e.g. forwarding it over a network API). The same as
/// [`connect_gige_as`] with [`Access::Control`].
pub async fn connect_gige_with_xml(
    device: &gige::DeviceInfo,
) -> Result<(Camera<GigeRegisterIo>, String), GenicamError> {
    connect_gige_as(device, Access::Control).await
}

// ---------------------------------------------------------------------------
// USB3 Vision transport (behind `u3v` feature)
// ---------------------------------------------------------------------------

/// Blocking [`RegisterIo`] adapter wrapping a [`U3vDevice`](u3v::U3vDevice).
///
/// Generic over `T: UsbTransfer` so that real hardware (`NusbTransfer`) and
/// test doubles (`MockUsbTransfer`, `FakeU3vTransport`) all work through the
/// same code path. USB operations are inherently synchronous, so this adapter
/// simply forwards calls through a `Mutex` for thread safety.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vRegisterIo<T: u3v::UsbTransfer + 'static> {
    /// Shared with a [`U3vFrameStream`](crate::U3vFrameStream), which clears
    /// StreamEnable through it when it stops.
    device: Arc<Mutex<u3v::U3vDevice<T>>>,
    /// Set once a transfer — a register access, a stream's, an event's —
    /// found the device disconnected.
    lost: Arc<std::sync::atomic::AtomicBool>,
}

/// Whether a USB3 Vision camera is gone, readable apart from its
/// transport ([`U3vRegisterIo::connection`]).
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
#[derive(Debug, Clone)]
pub struct U3vConnection(Arc<std::sync::atomic::AtomicBool>);

#[cfg(feature = "u3v")]
impl U3vConnection {
    /// [`U3vRegisterIo::is_lost`].
    pub fn is_lost(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[cfg(feature = "u3v")]
impl<T: u3v::UsbTransfer + 'static> U3vRegisterIo<T> {
    /// Create a new adapter wrapping a [`U3vDevice`](u3v::U3vDevice).
    pub fn new(device: u3v::U3vDevice<T>) -> Self {
        Self {
            device: Arc::new(Mutex::new(device)),
            lost: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Whether the camera is gone: a register access, a stream or an event
    /// stream on it failed with
    /// [`U3vError::Disconnected`](u3v::U3vError::Disconnected) — unplugged,
    /// or reset (backlog UV-03). It does not come back; open the camera
    /// again once it is.
    pub fn is_lost(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::Acquire)
    }

    /// A handle that answers [`is_lost`](Self::is_lost) without the
    /// transport: for a service watching a camera another task uses.
    pub fn connection(&self) -> U3vConnection {
        U3vConnection(Arc::clone(&self.lost))
    }

    /// Record the camera as gone.
    pub(crate) fn mark_lost(&self) {
        self.lost.store(true, std::sync::atomic::Ordering::Release);
    }

    /// The flag [`is_lost`](Self::is_lost) reads, for a stream to set.
    pub(crate) fn lost_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.lost)
    }

    /// Note a disconnect in `error`, and hand it on.
    fn noting_loss(&self, error: u3v::U3vError) -> u3v::U3vError {
        if matches!(error, u3v::U3vError::Disconnected) {
            self.mark_lost();
        }
        error
    }

    /// Lock the underlying device for direct access (e.g. stream configuration).
    pub fn lock_device(&self) -> Result<MutexGuard<'_, u3v::U3vDevice<T>>, GenicamError> {
        self.device
            .lock()
            .map_err(|_| GenicamError::Panicked("a thread holding the camera's device lock"))
    }

    /// The device, shared: a frame stream holds it to stop the stream.
    pub(crate) fn shared_device(&self) -> Arc<Mutex<u3v::U3vDevice<T>>> {
        Arc::clone(&self.device)
    }

    fn lock(&self) -> Result<MutexGuard<'_, u3v::U3vDevice<T>>, GenApiError> {
        self.device.lock().map_err(|_| {
            GenApiError::io(GenicamError::Panicked(
                "a thread holding the camera's device lock",
            ))
        })
    }
}

#[cfg(feature = "u3v")]
impl<T: u3v::UsbTransfer + 'static> RegisterIo for U3vRegisterIo<T> {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let mut device = self.lock()?;
        device
            .read_mem(addr, len)
            .map_err(|e| GenApiError::io(self.noting_loss(e)))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let mut device = self.lock()?;
        device
            .write_mem(addr, data)
            .map_err(|e| GenApiError::io(self.noting_loss(e)))
    }
}

/// What only a USB3 Vision camera has: an event interface, whose events
/// arrive on a USB endpoint rather than a GVCP message channel (backlog
/// TC-27 (4)). Its stream is
/// [`Camera::open_stream`](crate::Camera::open_stream) too.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
impl<T: u3v::UsbTransfer + 'static> Camera<U3vRegisterIo<T>> {
    /// Turn on notification of the events named — `"ExposureEnd"`, say —
    /// through SFNC's `EventSelector` and `EventNotification`. They arrive on
    /// a stream from [`open_event_stream`](Self::open_event_stream).
    pub fn configure_events(&mut self, enable_ids: &[&str]) -> Result<(), GenicamError> {
        self.enable_events(enable_ids)
    }

    /// Set the event interface's EventEnable and read its endpoint on a
    /// thread of its own. Events carry their device time in ns, mapped to
    /// host time once [`time_calibrate`](Camera::time_calibrate) has fitted
    /// a clock; dropping the stream clears EventEnable.
    pub fn open_event_stream(&self) -> Result<U3vEventStream, GenicamError> {
        let time_sync = (!self.time_sync.is_empty()).then(|| self.time_sync.clone());
        events::open_u3v_event_stream(&self.transport, time_sync)
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
/// let devices = u3v::discover()?;
/// let device = devices.into_iter().next().expect("no U3V camera found");
/// let mut camera = connect_u3v(&device)?;
/// camera.set("ExposureTime", "5000")?;
/// ```
#[cfg(feature = "u3v-usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v-usb")))]
pub fn connect_u3v(
    device: &u3v::U3vDeviceInfo,
) -> Result<Camera<U3vRegisterIo<u3v::NusbTransfer>>, GenicamError> {
    let (camera, _xml) = connect_u3v_with_xml(device)?;
    Ok(camera)
}

/// Connect to a USB3 Vision camera and return both a [`Camera`] and the raw
/// GenICam XML string fetched from the device.
#[cfg(feature = "u3v-usb")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v-usb")))]
pub fn connect_u3v_with_xml(
    device_info: &u3v::U3vDeviceInfo,
) -> Result<(Camera<U3vRegisterIo<u3v::NusbTransfer>>, String), GenicamError> {
    info!(
        vendor_id = device_info.vendor_id,
        product_id = device_info.product_id,
        "connecting to USB3 Vision camera"
    );

    let mut device =
        u3v::U3vDevice::open_device(device_info).map_err(error::u3v("open the device"))?;

    let xml = device.fetch_xml().map_err(error::u3v("fetch the XML"))?;

    let nodemap = build_nodemap(&xml)?;
    let transport = U3vRegisterIo::new(device);

    info!("USB3 Vision camera connected successfully");
    Ok((Camera::new(transport, nodemap), xml))
}

/// Create a [`Camera`] from an already-opened [`U3vDevice`](u3v::U3vDevice)
/// with any [`UsbTransfer`](u3v::UsbTransfer) backend.
///
/// This is the generic entry point for testing with fake or mock transports.
/// The device must have been opened and bootstrapped (ABRM/SBRM read)
/// before calling this function.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub fn open_u3v_device<T: u3v::UsbTransfer + 'static>(
    mut device: u3v::U3vDevice<T>,
) -> Result<(Camera<U3vRegisterIo<T>>, String), GenicamError> {
    let xml = device.fetch_xml().map_err(error::u3v("fetch the XML"))?;
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
    let model = viva_genapi_xml::parse(xml)?;
    // The nodemap absorbs the XML layer's losses, so one list covers both.
    let nodemap = NodeMap::try_from_xml(model)?;
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

    /// API-18 and SR-20: SFNC has no multicast "enable" — a multicast address
    /// in `GevSCDA` is what makes a stream multicast — and
    /// `GevSCPSDoNotFragment`, which the alias list used to name first, is
    /// the don't-fragment bit of `GevSCPSPacketSize`. The destination goes
    /// through the SFNC nodes, whose real name is `GevSCDA` (25 corpus
    /// documents; `GevSCPDA` is in none), and the port keeps the register's
    /// other half.
    #[test]
    fn multicast_goes_through_gevscda_and_leaves_do_not_fragment_alone() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="GevStreamChannelSelector"><pValue>SelectorReg</pValue></Integer>
                <IntReg Name="SelectorReg"><Address>0x2000</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
                <Integer Name="GevSCDA"><pValue>ScdaReg</pValue></Integer>
                <IntReg Name="ScdaReg"><Address>0x0D18</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></IntReg>
                <Integer Name="GevSCPHostPort"><pValue>ScpReg</pValue></Integer>
                <MaskedIntReg Name="ScpReg"><Address>0x0D00</Address><Length>4</Length><AccessMode>RW</AccessMode><LSB>31</LSB><MSB>16</MSB><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></MaskedIntReg>
                <Boolean Name="GevSCPSDoNotFragment"><pValue>DfReg</pValue></Boolean>
                <MaskedIntReg Name="DfReg"><Address>0x0D04</Address><Length>4</Length><AccessMode>RW</AccessMode><LSB>1</LSB><MSB>1</MSB><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess></MaskedIntReg>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MemoryIo::default();
        io.write(0x0D00, &0xABCD_0000u32.to_be_bytes()).unwrap();
        io.write(0x0D04, &1500u32.to_be_bytes()).unwrap();
        let mut camera = Camera::new(io, nodemap);

        let group = Ipv4Addr::new(239, 192, 1, 30);
        configure_multicast(&mut camera, 0, group, 50030).expect("configure");

        let read = |addr| camera.transport().read(addr, 4).unwrap();
        assert_eq!(read(0x0D18), group.octets());
        assert_eq!(
            read(0x0D00),
            0xABCD_C36Eu32.to_be_bytes(),
            "the port, written through GevSCPHostPort, keeps the other half"
        );
        assert_eq!(
            read(0x0D04),
            1500u32.to_be_bytes(),
            "the don't-fragment bit is untouched"
        );
    }

    /// An Allied Vision Manta G-125B declares `ChunkModeActive` — the
    /// stream channel's chunk bit, `0x0D24` bit 31 — and no `ChunkSelector`
    /// or `ChunkEnable`. Chunk mode alone is turned on and off; a selector
    /// asked for is what needs those features.
    #[test]
    fn chunk_mode_alone_needs_no_chunk_selector() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Boolean Name="ChunkModeActive" NameSpace="Standard">
                    <Streamable>Yes</Streamable>
                    <pValue>RegSCCFGChunk</pValue>
                    <OnValue>1</OnValue>
                    <OffValue>0</OffValue>
                </Boolean>
                <MaskedIntReg Name="RegSCCFGChunk" NameSpace="Custom">
                    <Address>0xD24</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <pPort>Device</pPort>
                    <Bit>31</Bit>
                    <Endianess>BigEndian</Endianess>
                </MaskedIntReg>
                <Port Name="Device" NameSpace="Standard"/>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let mut camera = Camera::new(MemoryIo::default(), nodemap);
        let mode = |active| ChunkConfig {
            selectors: Vec::new(),
            active,
        };

        camera.configure_chunks(&mode(true)).expect("chunk mode on");
        assert_eq!(camera.transport().read(0xD24, 4).unwrap(), [0, 0, 0, 1]);
        camera
            .configure_chunks(&mode(false))
            .expect("chunk mode off");
        assert_eq!(camera.transport().read(0xD24, 4).unwrap(), [0, 0, 0, 0]);

        let timestamp = ChunkConfig {
            selectors: vec!["Timestamp".into()],
            active: true,
        };
        assert!(matches!(
            camera.configure_chunks(&timestamp),
            Err(GenicamError::MissingChunkFeature(name)) if name == "ChunkSelector"
        ));
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

    /// GA-05: a command that declares a `<PollingTime>` is asked whether it
    /// has finished at that interval, not every `COMMAND_POLL_INTERVAL`
    /// (GenICam Standard v2.1.1 §2.8.8). Over 500 ms at 200 ms that is at
    /// most four reads; at 10 ms it would be fifty.
    #[tokio::test]
    async fn execute_and_wait_polls_at_the_commands_polling_time() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <Command Name="UserSetLoad">
                    <pValue>UserSetLoadReg</pValue>
                    <CommandValue>1</CommandValue>
                    <PollingTime>200</PollingTime>
                </Command>
                <IntReg Name="UserSetLoadReg">
                    <Address>0x4000</Address><Length>4</Length><AccessMode>RW</AccessMode>
                    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
                </IntReg>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let mut camera = Camera::new(SelfClearingIo::new(0x4000, usize::MAX), nodemap);
        let err = camera
            .execute_and_wait("UserSetLoad", Duration::from_millis(500))
            .await
            .expect_err("never clears");
        assert!(
            matches!(err, GenicamError::CommandNotDone { .. }),
            "{err:?}"
        );
        let (_, reads) = camera.transport().counts();
        assert!(
            reads <= 5,
            "{reads} reads in 500 ms at a 200 ms polling time"
        );
        assert!(reads >= 3, "{reads} reads: it did poll");
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

    /// Every kind of feature the typed accessors reach, over registers in
    /// memory.
    const TYPED_XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Category Name="Root"><pFeature>Width</pFeature></Category>
            <Integer Name="Width">
                <Address>0x10</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Min>1</Min><Max>4096</Max><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </Integer>
            <Float Name="Gain"><pValue>GainReg</pValue><Min>0</Min><Max>48</Max></Float>
            <FloatReg Name="GainReg">
                <Address>0x20</Address><Length>8</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess>
            </FloatReg>
            <Boolean Name="ReverseX"><pValue>ReverseXReg</pValue></Boolean>
            <IntReg Name="ReverseXReg">
                <Address>0x30</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <Enumeration Name="Mode">
                <EnumEntry Name="Off"><Value>0</Value></EnumEntry>
                <EnumEntry Name="On"><Value>1</Value></EnumEntry>
                <pValue>ModeReg</pValue>
            </Enumeration>
            <IntReg Name="ModeReg">
                <Address>0x40</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <StringReg Name="DeviceUserID">
                <Address>0x100</Address><Length>16</Length><AccessMode>RW</AccessMode>
            </StringReg>
            <Converter Name="FrameRate">
                <FormulaTo>FROM * 10</FormulaTo>
                <FormulaFrom>TO / 10</FormulaFrom>
                <pValue>FrameRateRaw</pValue>
                <Slope>Increasing</Slope>
            </Converter>
            <IntReg Name="FrameRateRaw">
                <Address>0x50</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <Command Name="AcquisitionStart">
                <pValue>AcquisitionStartReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="AcquisitionStartReg">
                <Address>0x60</Address><Length>4</Length><AccessMode>WO</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    fn memory_camera(xml: &str) -> Camera<MemoryIo> {
        let model = viva_genapi_xml::parse(xml).expect("parse");
        Camera::new(
            MemoryIo::default(),
            NodeMap::try_from_xml(model).expect("build nodemap"),
        )
    }

    /// API-05: each kind is read and written as its own type, as `NodeMap`
    /// does — a converter read as an integer is rounded, not refused because
    /// its text has a decimal point, and an integer is not a float.
    #[test]
    fn typed_accessors_reach_each_kind_as_its_own_type() {
        let mut camera = memory_camera(TYPED_XML);
        camera.set_integer("Width", 1024).expect("set Width");
        assert_eq!(camera.get_integer("Width").expect("Width"), 1024);
        camera.set_float("Gain", 1.0 / 3.0).expect("set Gain");
        assert_eq!(camera.get_float("Gain").expect("Gain"), 1.0 / 3.0);
        camera.set_bool("ReverseX", true).expect("set ReverseX");
        assert!(camera.get_bool("ReverseX").expect("ReverseX"));
        camera.set_enum("Mode", "On").expect("set Mode");
        assert_eq!(camera.get_enum("Mode").expect("Mode"), "On");
        camera
            .set_string("DeviceUserID", "left")
            .expect("set DeviceUserID");
        assert_eq!(camera.get_string("DeviceUserID").expect("id"), "left");

        camera
            .transport()
            .write(0x50, &123u32.to_be_bytes())
            .expect("raw frame rate");
        assert_eq!(camera.get_float("FrameRate").expect("float"), 12.3);
        assert_eq!(camera.get_integer("FrameRate").expect("integer"), 12);
        assert!(
            matches!(
                camera.get_float("Width"),
                Err(GenicamError::GenApi(GenApiError::Type { .. }))
            ),
            "an Integer is not read as a float"
        );
    }

    /// API-08: a category has no value to read, and a command is executed,
    /// not set — the value a `set` would have carried was ignored.
    #[test]
    fn get_on_a_category_and_set_on_a_command_are_errors() {
        let mut camera = memory_camera(TYPED_XML);
        assert!(camera.get("Root").is_err(), "a category has no value");
        let err = camera
            .set("AcquisitionStart", "1")
            .expect_err("a command is executed, not set");
        assert!(err.to_string().contains("execute_command"), "{err}");
        assert!(
            !camera.transport().0.lock().unwrap().contains_key(&0x60),
            "nothing was written"
        );
    }

    /// The names a camera without an SFNC 2 exposure or gain has instead:
    /// raw integers, and a category named `Gain` that is not the feature.
    const RAW_XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Category Name="Gain"><pFeature>GainRaw</pFeature></Category>
            <Integer Name="ExposureTimeRaw">
                <Address>0x10</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Min>1</Min><Max>100000</Max><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </Integer>
            <Integer Name="GainRaw">
                <Address>0x20</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Min>0</Min><Max>1023</Max><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </Integer>
        </RegisterDescription>
    "#;

    /// API-14: exposure and gain fall back to the raw integer features, a
    /// value rounded to the integer it must be, and say which they wrote;
    /// a category that shares the name `Gain` is passed over.
    #[test]
    fn exposure_and_gain_fall_back_to_raw_integers() {
        let mut camera = memory_camera(RAW_XML);
        let exposure = camera.set_exposure_time(250.4).expect("exposure");
        assert_eq!(
            (exposure.name, exposure.kind),
            ("ExposureTimeRaw", "Integer")
        );
        assert_eq!(
            camera.transport().read(0x10, 4).unwrap(),
            250u32.to_be_bytes()
        );
        assert_eq!(camera.exposure_time().expect("read back"), 250.0);

        let gain = camera.set_gain(3.0).expect("gain");
        assert_eq!((gain.name, gain.kind), ("GainRaw", "Integer"));
        assert_eq!(
            camera.transport().read(0x20, 4).unwrap(),
            3u32.to_be_bytes()
        );
    }

    /// API-14: a feature the camera has under none of its names is an error
    /// that lists every name tried.
    #[test]
    fn a_feature_the_camera_lacks_names_every_candidate() {
        let mut camera = memory_camera(TYPED_XML);
        let err = camera.set_exposure_time(1.0).expect_err("no exposure");
        assert!(
            err.to_string()
                .contains("ExposureTime, ExposureTimeAbs, ExposureTimeRaw"),
            "{err}"
        );
    }

    /// Binning as the Hikrobot MV-CS050-10GC declares it: an enumeration
    /// whose entry names end in the factor (vertically an integer here).
    const BINNING_XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Enumeration Name="BinningHorizontal">
                <EnumEntry Name="BinningHorizontal1"><Value>1</Value></EnumEntry>
                <EnumEntry Name="BinningHorizontal2"><Value>2</Value></EnumEntry>
                <EnumEntry Name="BinningHorizontal4"><Value>4</Value></EnumEntry>
                <pValue>BinningHorizontalReg</pValue>
            </Enumeration>
            <IntReg Name="BinningHorizontalReg">
                <Address>0x10</Address><Length>4</Length><AccessMode>RW</AccessMode><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="BinningVertical">
                <Address>0x20</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Min>1</Min><Max>4</Max><Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </Integer>
        </RegisterDescription>
    "#;

    /// API-14: an enumerated binning is written by the entry whose name ends
    /// in the factor.
    #[test]
    fn binning_by_enumeration_writes_the_entry_naming_the_factor() {
        let mut camera = memory_camera(BINNING_XML);
        camera.set_binning(2, 2).expect("binning");
        assert_eq!(
            camera.transport().read(0x10, 4).unwrap(),
            2u32.to_be_bytes()
        );
        assert_eq!(
            camera.transport().read(0x20, 4).unwrap(),
            2u32.to_be_bytes()
        );
        assert_eq!(camera.binning().expect("read back"), (2, 2));
    }

    /// API-14: SVS's evo4050 declares `BinningHorizontal` as `Off`/`On`,
    /// which names no factor: refused, with the entries listed, and nothing
    /// written.
    #[test]
    fn binning_by_enumeration_without_factors_is_refused() {
        let xml = BINNING_XML
            .replace(
                r#"<EnumEntry Name="BinningHorizontal1"><Value>1</Value></EnumEntry>"#,
                r#"<EnumEntry Name="Off"><Value>0</Value></EnumEntry>"#,
            )
            .replace(
                r#"<EnumEntry Name="BinningHorizontal2"><Value>2</Value></EnumEntry>"#,
                r#"<EnumEntry Name="On"><Value>1</Value></EnumEntry>"#,
            )
            .replace(
                r#"<EnumEntry Name="BinningHorizontal4"><Value>4</Value></EnumEntry>"#,
                "",
            );
        let mut camera = memory_camera(&xml);
        let err = camera
            .set_binning(2, 2)
            .expect_err("Off/On names no factor");
        assert!(err.to_string().contains("Off, On"), "{err}");
        assert!(
            camera.transport().0.lock().unwrap().is_empty(),
            "nothing was written"
        );
    }
}
