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
//!     channel: 0,
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
pub mod frame;
pub mod stream;
pub mod time;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::events::{
    bind_socket as bind_event_socket_internal,
    configure_message_channel_raw as configure_message_channel_fallback,
    enable_event_raw as enable_event_fallback, parse_event_id,
};
use crate::genapi::{GenApiError, Node, NodeMap, RegisterIo, SkOutput};
use gige::GigeDevice;
use gige::gvcp::consts as gvcp_consts;
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info, warn};

pub use chunks::{ChunkKind, ChunkMap, ChunkValue, parse_chunk_bytes};
pub use events::{Event, EventStream};
pub use frame::Frame;
pub use gige::action::{AckSummary, ActionParams};
pub use stream::{FrameStream, Stream, StreamBuilder, StreamDest};
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
}

impl GenicamError {
    fn parse<S: Into<String>>(msg: S) -> Self {
        GenicamError::Parse(msg.into())
    }

    fn transport<S: Into<String>>(msg: S) -> Self {
        GenicamError::Transport(msg.into())
    }
}

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
            Some(Node::Category(_)) => Ok(String::new()),
            None => Err(GenApiError::NodeNotFound(name.to_string()).into()),
        }
    }

    /// Set a feature value using a string representation.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), GenicamError> {
        match self.nodemap.node(name) {
            Some(Node::Integer(_)) => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| GenicamError::parse(format!("invalid integer for {name}")))?;
                self.nodemap
                    .set_integer(name, parsed, &self.transport)
                    .map_err(Into::into)
            }
            Some(Node::Float(_)) => {
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
            Some(Node::Converter(_)) => {
                // Converters are read-only from the user perspective
                // (they transform values from underlying nodes)
                Err(GenApiError::Type(name.to_string()).into())
            }
            Some(Node::IntConverter(_)) => Err(GenApiError::Type(name.to_string()).into()),
            Some(Node::String(_)) => self
                .nodemap
                .set_string(name, value, &self.transport)
                .map_err(Into::into),
            Some(Node::Command(_)) => self
                .nodemap
                .exec_command(name, &self.transport)
                .map_err(Into::into),
            Some(Node::Category(_)) => Err(GenApiError::Type(name.to_string()).into()),
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

    /// Map device tick counters to host time using the fitted model.
    pub fn map_dev_ts(&self, dev_ticks: u64) -> SystemTime {
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

        if !used_sfnc {
            for &name in enable_ids {
                let Some(event_id) = parse_event_id(name) else {
                    return Err(GenicamError::transport(format!(
                        "event '{name}' missing from nodemap and not numeric"
                    )));
                };
                enable_event_fallback(&self.transport, event_id, true)?;
            }
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
/// The adapter uses [`tokio::task::block_in_place`] combined with
/// [`tokio::runtime::Handle::block_on`] to synchronously wait on GVCP register
/// transactions. This is safe to call from both async and sync contexts.
pub struct GigeRegisterIo {
    handle: tokio::runtime::Handle,
    device: Mutex<GigeDevice>,
}

impl GigeRegisterIo {
    /// Create a new adapter using the provided runtime handle and device.
    pub fn new(handle: tokio::runtime::Handle, device: GigeDevice) -> Self {
        Self {
            handle,
            device: Mutex::new(device),
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

impl RegisterIo for GigeRegisterIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let mut device = self.lock()?;
        tokio::task::block_in_place(|| self.handle.block_on(device.read_mem(addr, len)))
            .map_err(|err| GenApiError::Io(err.to_string()))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        let mut device = self.lock()?;
        tokio::task::block_in_place(|| self.handle.block_on(device.write_mem(addr, data)))
            .map_err(|err| GenApiError::Io(err.to_string()))
    }
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

    let model = viva_genapi_xml::parse(&xml).map_err(|e| GenicamError::transport(e.to_string()))?;
    let nodemap = genapi::NodeMap::from(model);

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
    device: Mutex<u3v::device::U3vDevice<T>>,
}

#[cfg(feature = "u3v")]
impl<T: u3v::usb::UsbTransfer + 'static> U3vRegisterIo<T> {
    /// Create a new adapter wrapping a [`U3vDevice`](u3v::device::U3vDevice).
    pub fn new(device: u3v::device::U3vDevice<T>) -> Self {
        Self {
            device: Mutex::new(device),
        }
    }

    /// Lock the underlying device for direct access (e.g. stream configuration).
    pub fn lock_device(&self) -> Result<MutexGuard<'_, u3v::device::U3vDevice<T>>, GenicamError> {
        self.device
            .lock()
            .map_err(|_| GenicamError::transport("u3v device mutex poisoned"))
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

    let model = viva_genapi_xml::parse(&xml).map_err(|e| GenicamError::transport(e.to_string()))?;
    let nodemap = genapi::NodeMap::from(model);
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
    let model = viva_genapi_xml::parse(&xml).map_err(|e| GenicamError::transport(e.to_string()))?;
    let nodemap = genapi::NodeMap::from(model);
    let transport = U3vRegisterIo::new(device);
    Ok((Camera::new(transport, nodemap), xml))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}
