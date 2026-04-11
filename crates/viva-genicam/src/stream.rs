//! Streaming builder and configuration helpers bridging `tl-gige` with
//! higher-level GenICam consumers.
//!
//! The builder performs control-plane negotiation (packet size, delay) and
//! prepares a UDP socket configured for reception. Applications can retrieve the
//! socket handle to drive their own async pipelines while relying on the shared
//! [`StreamStats`] accumulator for monitoring.
//!
//! # High-Level Streaming
//!
//! For most use cases, [`FrameStream`] provides an ergonomic async iterator over
//! reassembled frames:
//!
//! ```rust,ignore
//! let stream = FrameStream::new(raw_stream, None);
//! while let Some(frame) = stream.next_frame().await? {
//!     println!("{}x{} frame", frame.width, frame.height);
//! }
//! ```

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant, SystemTime};

use bytes::{Bytes, BytesMut};
use tokio::net::UdpSocket;
use tracing::{debug, info, trace, warn};
use viva_pfnc::PixelFormat;

use crate::GenicamError;
use crate::frame::Frame;
use crate::time::TimeSync;
use viva_gige::gvcp::{GigeDevice, StreamParams};
use viva_gige::gvsp::{self, GvspPacket, PacketBitmap, StreamConfig};
use viva_gige::nic::{self, DEFAULT_RCVBUF_BYTES, Iface, McOptions};
use viva_gige::stats::{StreamStats, StreamStatsAccumulator};

pub use viva_gige::gvsp::StreamDest;

/// Internal packet source abstraction.
///
/// Holds either a standard UDP socket or a custom transport backend.
/// This avoids making `Stream`/`FrameStream` generic while supporting
/// both paths.
pub(crate) enum PacketSource {
    Udp(UdpSocket),
}

impl PacketSource {
    /// Receive raw packet bytes from the source.
    async fn recv(&self, buf: &mut [u8]) -> Result<Bytes, GenicamError> {
        match self {
            PacketSource::Udp(socket) => {
                let (len, _) = socket
                    .recv_from(buf)
                    .await
                    .map_err(|e| GenicamError::transport(format!("socket recv failed: {e}")))?;
                Ok(Bytes::copy_from_slice(&buf[..len]))
            }
        }
    }

    /// Borrow the UDP socket, if this is the UDP path.
    fn as_udp_socket(&self) -> Option<&UdpSocket> {
        match self {
            PacketSource::Udp(s) => Some(s),
        }
    }
}

/// Builder for configuring a GVSP stream.
pub struct StreamBuilder<'a> {
    device: &'a mut GigeDevice,
    iface: Option<Iface>,
    dest: Option<StreamDest>,
    rcvbuf_bytes: Option<usize>,
    auto_packet_size: bool,
    target_mtu: Option<u32>,
    packet_size: Option<u32>,
    packet_delay: Option<u32>,
    channel: u32,
    dst_port: u16,
}

impl<'a> StreamBuilder<'a> {
    /// Create a new builder bound to an opened [`GigeDevice`].
    pub fn new(device: &'a mut GigeDevice) -> Self {
        Self {
            device,
            iface: None,
            dest: None,
            rcvbuf_bytes: None,
            auto_packet_size: true,
            target_mtu: None,
            packet_size: None,
            packet_delay: None,
            channel: 0,
            dst_port: 0,
        }
    }

    /// Select the interface used for receiving GVSP packets.
    pub fn iface(mut self, iface: Iface) -> Self {
        self.iface = Some(iface);
        self
    }

    /// Configure the stream destination.
    pub fn dest(mut self, dest: StreamDest) -> Self {
        self.dest = Some(dest);
        self
    }

    /// Enable or disable automatic packet-size negotiation.
    pub fn auto_packet_size(mut self, enable: bool) -> Self {
        self.auto_packet_size = enable;
        self
    }

    /// Target MTU used when computing the optimal GVSP packet size.
    pub fn target_mtu(mut self, mtu: u32) -> Self {
        self.target_mtu = Some(mtu);
        self
    }

    /// Override the GVSP packet size when automatic negotiation is disabled.
    pub fn packet_size(mut self, size: u32) -> Self {
        self.packet_size = Some(size);
        self
    }

    /// Override the GVSP packet delay when automatic negotiation is disabled.
    pub fn packet_delay(mut self, delay: u32) -> Self {
        self.packet_delay = Some(delay);
        self
    }

    /// Configure the UDP port used for streaming (defaults to 0 => device chosen).
    pub fn destination_port(mut self, port: u16) -> Self {
        self.dst_port = port;
        if let Some(dest) = &mut self.dest {
            *dest = match *dest {
                StreamDest::Unicast { dst_ip, .. } => StreamDest::Unicast {
                    dst_ip,
                    dst_port: port,
                },
                StreamDest::Multicast {
                    group,
                    loopback,
                    ttl,
                    ..
                } => StreamDest::Multicast {
                    group,
                    port,
                    loopback,
                    ttl,
                },
            };
        }
        self
    }

    /// Configure multicast reception when the device is set to multicast mode.
    pub fn multicast(mut self, group: Option<Ipv4Addr>) -> Self {
        if let Some(group) = group {
            self.dest = Some(StreamDest::Multicast {
                group,
                port: self.dst_port,
                loopback: false,
                ttl: 1,
            });
        } else {
            self.dest = None;
        }
        self
    }

    /// Custom receive buffer size for the UDP socket.
    pub fn rcvbuf_bytes(mut self, size: usize) -> Self {
        self.rcvbuf_bytes = Some(size);
        self
    }

    /// Select the GigE Vision stream channel to configure (defaults to 0).
    pub fn channel(mut self, channel: u32) -> Self {
        self.channel = channel;
        self
    }

    /// Finalise the builder and return a configured [`Stream`].
    pub async fn build(self) -> Result<Stream, GenicamError> {
        let iface = self
            .iface
            .ok_or_else(|| GenicamError::transport("stream requires a network interface"))?;
        let host_ip = iface
            .ipv4()
            .ok_or_else(|| GenicamError::transport("interface lacks IPv4 address"))?;
        let default_port = if self.dst_port == 0 {
            0x5FFF
        } else {
            self.dst_port
        };
        let mut dest = self.dest.unwrap_or(StreamDest::Unicast {
            dst_ip: host_ip,
            dst_port: default_port,
        });
        match &mut dest {
            StreamDest::Unicast { dst_port, .. } => {
                if *dst_port == 0 {
                    *dst_port = default_port;
                }
            }
            StreamDest::Multicast { port, .. } => {
                if *port == 0 {
                    *port = default_port;
                }
            }
        }

        let iface_mtu = nic::mtu(&iface).map_err(|err| GenicamError::transport(err.to_string()))?;
        let mtu = self
            .target_mtu
            .map_or(iface_mtu, |limit| limit.min(iface_mtu));
        let packet_size = if self.auto_packet_size {
            nic::best_packet_size(mtu)
        } else {
            self.packet_size
                .unwrap_or_else(|| nic::best_packet_size(1500))
        };
        let packet_delay = if self.auto_packet_size {
            if mtu <= 1500 {
                const DELAY_NS: u32 = 2_000;
                DELAY_NS / 80
            } else {
                0
            }
        } else {
            self.packet_delay.unwrap_or(0)
        };

        match &dest {
            StreamDest::Unicast { dst_ip, dst_port } => {
                info!(%dst_ip, dst_port, channel = self.channel, "configuring unicast stream");
                self.device
                    .set_stream_destination(self.channel, *dst_ip, *dst_port)
                    .await
                    .map_err(|err| GenicamError::transport(err.to_string()))?;
            }
            StreamDest::Multicast { .. } => {
                info!(
                    channel = self.channel,
                    port = dest.port(),
                    addr = %dest.addr(),
                    "configuring multicast stream parameters"
                );
            }
        }

        self.device
            .set_stream_packet_size(self.channel, packet_size)
            .await
            .map_err(|err| GenicamError::transport(err.to_string()))?;
        self.device
            .set_stream_packet_delay(self.channel, packet_delay)
            .await
            .map_err(|err| GenicamError::transport(err.to_string()))?;

        let source = PacketSource::Udp(Self::bind_socket(&dest, &iface, self.rcvbuf_bytes).await?);

        let source_filter = if dest.is_multicast() {
            None
        } else {
            Some(dest.addr())
        };
        let resend_enabled = !dest.is_multicast();

        let params = StreamParams {
            packet_size,
            packet_delay,
            mtu,
            host: dest.addr(),
            port: dest.port(),
        };

        let config = StreamConfig {
            dest,
            iface: iface.clone(),
            packet_size: Some(packet_size),
            packet_delay: Some(packet_delay),
            source_filter,
            resend_enabled,
        };

        let stats = StreamStatsAccumulator::new();
        Ok(Stream {
            source,
            stats,
            params,
            config,
        })
    }

    /// Bind a UDP socket for the given stream destination.
    async fn bind_socket(
        dest: &StreamDest,
        iface: &Iface,
        rcvbuf_bytes: Option<usize>,
    ) -> Result<UdpSocket, GenicamError> {
        match dest {
            StreamDest::Unicast { dst_port, .. } => {
                let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
                nic::bind_udp(bind_ip, *dst_port, Some(iface.clone()), rcvbuf_bytes)
                    .await
                    .map_err(|err| GenicamError::transport(err.to_string()))
            }
            StreamDest::Multicast {
                group,
                port,
                loopback,
                ttl,
            } => {
                let opts = McOptions {
                    loopback: *loopback,
                    ttl: *ttl,
                    rcvbuf_bytes: rcvbuf_bytes.unwrap_or(DEFAULT_RCVBUF_BYTES),
                    ..McOptions::default()
                };
                nic::bind_multicast(iface, *group, *port, &opts)
                    .await
                    .map_err(|err| GenicamError::transport(err.to_string()))
            }
        }
    }
}

/// Handle returned by [`StreamBuilder`] providing access to the configured
/// packet source and statistics.
pub struct Stream {
    source: PacketSource,
    stats: StreamStatsAccumulator,
    params: StreamParams,
    config: StreamConfig,
}

impl Stream {
    /// Borrow the underlying UDP socket (returns `None` when using a custom transport).
    pub fn socket(&self) -> Option<&UdpSocket> {
        self.source.as_udp_socket()
    }

    /// Consume the stream and return its parts.
    pub(crate) fn into_parts(
        self,
    ) -> (
        PacketSource,
        StreamStatsAccumulator,
        StreamParams,
        StreamConfig,
    ) {
        (self.source, self.stats, self.params, self.config)
    }

    /// Access the negotiated stream parameters.
    pub fn params(&self) -> StreamParams {
        self.params
    }

    /// Obtain a clone of the statistics accumulator handle for updates.
    pub fn stats_handle(&self) -> StreamStatsAccumulator {
        self.stats.clone()
    }

    /// Snapshot the collected statistics.
    pub fn stats(&self) -> StreamStats {
        self.stats.snapshot()
    }

    /// Immutable view of the stream configuration.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

impl<'a> From<&'a mut GigeDevice> for StreamBuilder<'a> {
    fn from(device: &'a mut GigeDevice) -> Self {
        StreamBuilder::new(device)
    }
}

// ============================================================================
// High-Level FrameStream API
// ============================================================================

/// Default timeout for frame assembly before declaring incomplete and moving on.
const DEFAULT_FRAME_TIMEOUT: Duration = Duration::from_millis(100);

/// GVSP header size preceding payload data.
const GVSP_HEADER_SIZE: usize = 8;

/// State for a frame being assembled from GVSP packets.
#[derive(Debug)]
struct FrameAssemblyState {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    expected_packets: Option<usize>,
    bitmap: Option<PacketBitmap>,
    payload: BytesMut,
    packet_payload_size: usize,
    started: Instant,
}

impl FrameAssemblyState {
    fn new(
        block_id: u64,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        timestamp: u64,
        packet_payload_size: usize,
    ) -> Self {
        Self {
            block_id,
            width,
            height,
            pixel_format,
            timestamp,
            expected_packets: None,
            bitmap: None,
            payload: BytesMut::new(),
            packet_payload_size,
            started: Instant::now(),
        }
    }

    /// Ingest a payload packet. Returns true if this is a new packet.
    fn ingest(&mut self, packet_id: u32, data: &[u8]) -> bool {
        let pid = packet_id as usize;

        // Track received packets if we know the total count.
        if let Some(ref mut bitmap) = self.bitmap {
            if !bitmap.set(pid) {
                return false; // Duplicate packet.
            }
        }

        // Write data at the correct offset for zero-copy reassembly.
        let offset = pid.saturating_sub(1) * self.packet_payload_size;
        let required = offset + data.len();
        if self.payload.len() < required {
            self.payload.resize(required, 0);
        }
        self.payload[offset..offset + data.len()].copy_from_slice(data);
        true
    }

    /// Set the expected packet count (from trailer packet_id + 1).
    fn set_expected_packets(&mut self, count: usize) {
        if self.expected_packets.is_none() {
            self.expected_packets = Some(count);
            self.bitmap = Some(PacketBitmap::new(count));
        }
    }

    /// Check if all packets have been received.
    #[allow(dead_code)]
    fn is_complete(&self) -> bool {
        self.bitmap.as_ref().is_some_and(|b| b.is_complete())
    }

    /// Check if assembly has timed out.
    fn is_expired(&self, timeout: Duration) -> bool {
        self.started.elapsed() > timeout
    }

    /// Get missing packet ranges for resend requests.
    #[allow(dead_code)]
    fn missing_ranges(&self) -> Vec<std::ops::RangeInclusive<u32>> {
        self.bitmap
            .as_ref()
            .map(|b| b.missing_ranges())
            .unwrap_or_default()
    }
}

/// High-level async iterator over reassembled GVSP frames.
///
/// Wraps a low-level [`Stream`] and handles packet parsing, reassembly,
/// and optional resend requests automatically.
///
/// # Example
///
/// ```rust,ignore
/// let raw_stream = StreamBuilder::new(&mut device)
///     .iface(iface)
///     .build()
///     .await?;
/// let mut frame_stream = FrameStream::new(raw_stream, None);
/// while let Some(frame) = frame_stream.next_frame().await? {
///     println!("Frame: {}x{}", frame.width, frame.height);
/// }
/// ```
pub struct FrameStream {
    source: PacketSource,
    stats: StreamStatsAccumulator,
    params: StreamParams,
    config: StreamConfig,
    recv_buffer: Vec<u8>,
    active: Option<FrameAssemblyState>,
    frame_timeout: Duration,
    time_sync: Option<TimeSync>,
}

impl FrameStream {
    /// Create a new frame stream from a configured [`Stream`].
    ///
    /// Optionally accepts a [`TimeSync`] for mapping device timestamps to host time.
    pub fn new(stream: Stream, time_sync: Option<TimeSync>) -> Self {
        let (source, stats, params, config) = stream.into_parts();
        let buffer_size = (params.packet_size as usize + 64).max(4096);

        Self {
            source,
            stats,
            params,
            config,
            recv_buffer: vec![0u8; buffer_size],
            active: None,
            frame_timeout: DEFAULT_FRAME_TIMEOUT,
            time_sync,
        }
    }

    /// Set the frame assembly timeout.
    ///
    /// If a frame is not complete within this duration, it will be dropped
    /// and assembly will move on to the next frame.
    pub fn set_frame_timeout(&mut self, timeout: Duration) {
        self.frame_timeout = timeout;
    }

    /// Update or set the time synchronization model for timestamp mapping.
    pub fn set_time_sync(&mut self, time_sync: TimeSync) {
        self.time_sync = Some(time_sync);
    }

    /// Obtain a clone of the statistics accumulator handle.
    pub fn stats_handle(&self) -> StreamStatsAccumulator {
        self.stats.clone()
    }

    /// Snapshot the collected statistics.
    pub fn stats(&self) -> StreamStats {
        self.stats.snapshot()
    }

    /// Access the negotiated stream parameters.
    pub fn params(&self) -> StreamParams {
        self.params
    }

    /// Immutable view of the stream configuration.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// Borrow the underlying UDP socket (returns `None` when using a custom transport).
    pub fn socket(&self) -> Option<&UdpSocket> {
        self.source.as_udp_socket()
    }

    /// Receive the next complete frame.
    ///
    /// This method handles packet reception, parsing, and reassembly internally.
    /// Returns `Ok(Some(frame))` when a complete frame is available, or
    /// `Ok(None)` if the stream has ended (socket closed).
    pub async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        loop {
            // Check for timeout on active frame assembly.
            if let Some(ref active) = self.active {
                if active.is_expired(self.frame_timeout) {
                    let block_id = active.block_id;
                    warn!(
                        block_id,
                        "frame assembly timeout, dropping incomplete frame"
                    );
                    self.stats.record_drop();
                    self.active = None;
                }
            }

            // Receive next packet.
            let raw = match self.source.recv(&mut self.recv_buffer).await {
                Ok(data) if data.is_empty() => return Ok(None), // Stream closed.
                Ok(data) => data,
                Err(e) => return Err(e),
            };

            // Parse GVSP packet.
            let packet = match gvsp::parse_packet(&raw) {
                Ok(p) => p,
                Err(e) => {
                    trace!(error = %e, "discarding malformed GVSP packet");
                    continue;
                }
            };

            // Process packet based on type.
            match packet {
                GvspPacket::Leader {
                    block_id,
                    width,
                    height,
                    pixel_format,
                    timestamp,
                    ..
                } => {
                    // Start new frame assembly, dropping any incomplete previous frame.
                    if let Some(ref prev) = self.active {
                        if prev.block_id != block_id {
                            debug!(
                                old_block = prev.block_id,
                                new_block = block_id,
                                "new leader arrived, dropping incomplete frame"
                            );
                            self.stats.record_drop();
                        }
                    }

                    let pixel_format = PixelFormat::from_code(pixel_format);
                    let packet_payload = self.params.packet_size as usize - GVSP_HEADER_SIZE;

                    self.active = Some(FrameAssemblyState::new(
                        block_id,
                        width,
                        height,
                        pixel_format,
                        timestamp,
                        packet_payload,
                    ));
                    trace!(block_id, %pixel_format, width, height, "frame leader received");
                }

                GvspPacket::Payload {
                    block_id,
                    packet_id,
                    data,
                } => {
                    if let Some(ref mut active) = self.active {
                        if active.block_id == block_id && active.ingest(packet_id, data.as_ref()) {
                            self.stats.record_packet();
                        }
                    }
                }

                GvspPacket::Trailer {
                    block_id,
                    packet_id,
                    status,
                    chunk_data,
                } => {
                    let Some(mut active) = self.active.take() else {
                        continue;
                    };

                    if active.block_id != block_id {
                        // Mismatched trailer, drop and continue.
                        self.stats.record_drop();
                        continue;
                    }

                    // Set expected packet count from trailer packet_id.
                    // Trailer packet_id is the last packet index, so total = packet_id + 1.
                    // But packet_id 0 is leader, so payload packets = packet_id.
                    active.set_expected_packets(packet_id as usize);

                    if status != 0 {
                        warn!(block_id, status, "trailer reported non-zero status");
                    }

                    // Build the frame.
                    let ts_host = self
                        .time_sync
                        .as_ref()
                        .map(|ts| ts.to_host_time(active.timestamp));

                    let chunks = if chunk_data.is_empty() {
                        None
                    } else {
                        match crate::chunks::parse_chunk_bytes(&chunk_data) {
                            Ok(map) => Some(map),
                            Err(e) => {
                                debug!(error = %e, "failed to parse chunk data");
                                None
                            }
                        }
                    };

                    // Truncate payload to actual received size.
                    // The bitmap tells us what we received; we use the payload as-is.
                    let payload = active.payload.freeze();

                    let frame = Frame {
                        payload,
                        width: active.width,
                        height: active.height,
                        pixel_format: active.pixel_format,
                        chunks,
                        ts_dev: Some(active.timestamp),
                        ts_host,
                    };

                    // Record statistics.
                    let latency = frame
                        .host_time()
                        .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                    self.stats.record_frame(frame.payload.len(), latency);

                    debug!(
                        block_id,
                        width = frame.width,
                        height = frame.height,
                        bytes = frame.payload.len(),
                        "frame complete"
                    );

                    return Ok(Some(frame));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_assembly_state_ingest_tracks_packets() {
        let mut state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);
        state.set_expected_packets(3);

        // Ingest packets (packet_id 1 and 2 are payload, 0 is leader).
        assert!(state.ingest(1, &[1, 2, 3]));
        assert!(state.ingest(2, &[4, 5, 6]));

        // Duplicate should return false.
        assert!(!state.ingest(1, &[1, 2, 3]));
    }

    #[test]
    fn frame_assembly_state_timeout() {
        let state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);
        assert!(!state.is_expired(Duration::from_secs(10)));
        assert!(state.is_expired(Duration::ZERO));
    }
}
