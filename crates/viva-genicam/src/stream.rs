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

#[cfg(any(not(windows), test))]
use std::collections::HashSet;
#[cfg(windows)]
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(windows)]
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use bytes::{Bytes, BytesMut};
use tokio::net::UdpSocket;
// Used by the receive loop on non-Windows and by the packet-size probe
// everywhere — the probe runs before the socket is handed to the Windows
// reader thread, so it is a tokio socket on every platform.
use tokio::time::timeout;
#[cfg(not(windows))]
use tracing::trace;
use tracing::{debug, info, warn};
use viva_pfnc::PixelFormat;

use crate::GenicamError;
use crate::chunks::ChunkMap;
use crate::evs::EvsBlock;
use crate::frame::Frame;
use crate::time::TimeSync;
use viva_gige::gvcp::{GigeDevice, StreamParams};
#[cfg(any(not(windows), test))]
use viva_gige::gvsp::PacketBitmap;
use viva_gige::gvsp::{self, GvspPacket, StreamConfig};
use viva_gige::nic::{self, DEFAULT_RCVBUF_BYTES, Iface, McOptions};
use viva_gige::stats::{StreamStats, StreamStatsAccumulator};

pub use viva_gige::gvsp::StreamDest;

/// The semantic payload carried by one complete GVSP block.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum StreamBlock {
    /// A conventional image payload.
    Image(Frame),
    /// A LUCID event-vision payload encoded as EVT 2.1 or EVT 3.0.
    Evs(EvsBlock),
}

/// An opaque complete GVSP block with common metadata accessors.
///
/// Convert this value into [`StreamBlock`] to inspect the image- or EVS-specific
/// representation. Keeping the inner field private leaves room for adding
/// common behavior without exposing storage details here.
#[derive(Debug, Clone)]
pub struct GenericStreamBlock {
    inner: StreamBlock,
}

impl From<GenericStreamBlock> for StreamBlock {
    fn from(value: GenericStreamBlock) -> Self {
        value.inner
    }
}

impl GenericStreamBlock {
    /// Borrow the complete application payload.
    pub fn payload(&self) -> &Bytes {
        match &self.inner {
            StreamBlock::Image(frame) => &frame.payload,
            StreamBlock::Evs(block) => &block.payload,
        }
    }

    /// Host-correlated timestamp, when time synchronization is available.
    pub fn host_time(&self) -> Option<SystemTime> {
        match &self.inner {
            StreamBlock::Image(frame) => frame.ts_host,
            StreamBlock::Evs(block) => block.ts_host,
        }
    }

    /// Image width, or `None` for a payload without image geometry.
    pub fn width(&self) -> Option<u32> {
        match &self.inner {
            StreamBlock::Image(frame) => Some(frame.width),
            StreamBlock::Evs(_) => None,
        }
    }

    /// Image height, or `None` for a payload without image geometry.
    pub fn height(&self) -> Option<u32> {
        match &self.inner {
            StreamBlock::Image(frame) => Some(frame.height),
            StreamBlock::Evs(_) => None,
        }
    }

    #[cfg(windows)]
    fn device_timestamp(&self) -> Option<u64> {
        match &self.inner {
            StreamBlock::Image(frame) => frame.ts_dev,
            StreamBlock::Evs(block) => block.ts_dev,
        }
    }

    #[cfg(windows)]
    fn set_host_time(&mut self, timestamp: Option<SystemTime>) {
        match &mut self.inner {
            StreamBlock::Image(frame) => frame.ts_host = timestamp,
            StreamBlock::Evs(block) => block.ts_host = timestamp,
        }
    }

    fn into_legacy_frame(self) -> Frame {
        match self.inner {
            StreamBlock::Image(frame) => frame,
            StreamBlock::Evs(block) => Frame {
                payload: block.payload,
                width: block.leader_size_x,
                height: block.leader_size_y,
                pixel_format: block.format.pixel_format(),
                chunks: block.chunks,
                ts_dev: block.ts_dev,
                ts_host: block.ts_host,
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn classify_completed_block(
    payload: Bytes,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    chunks: Option<ChunkMap>,
    timestamp: u64,
    ts_host: Option<SystemTime>,
    trailer_size_y: u32,
) -> GenericStreamBlock {
    let inner = match pixel_format {
        PixelFormat::EvsEvt30 => StreamBlock::Evs(EvsBlock {
            payload,
            format: crate::evs::EvsFormat::Evt30,
            ts_dev: Some(timestamp),
            leader_size_x: width,
            leader_size_y: height,
            trailer_size_y,
            chunks,
            ts_host,
        }),
        PixelFormat::EvsEvt21 => StreamBlock::Evs(EvsBlock {
            payload,
            format: crate::evs::EvsFormat::Evt21,
            ts_dev: Some(timestamp),
            leader_size_x: width,
            leader_size_y: height,
            trailer_size_y,
            chunks,
            ts_host,
        }),
        _ => StreamBlock::Image(Frame {
            payload,
            width,
            height,
            pixel_format,
            chunks,
            ts_dev: Some(timestamp),
            ts_host,
        }),
    };
    GenericStreamBlock { inner }
}

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
    #[cfg(not(windows))]
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

/// Smallest GVSP packet size we will configure: the IPv4 minimum reassembly
/// buffer.
const MIN_PACKET_SIZE: u32 = 576;

/// Largest GVSP packet size we will configure.
///
/// Both bounds bite at once: an IPv4 datagram cannot exceed 65 535 bytes, and
/// `GevSCPSPacketSize` holds the size in 16 bits.
const MAX_PACKET_SIZE: u32 = viva_gige::gvcp::STREAM_PACKET_SIZE_MASK;

/// Write `GevSCPSPacketSize`, then read it back and return what the device
/// actually holds.
///
/// A camera may clamp the requested size to what it supports, and the write
/// succeeds when it does — nothing on the wire distinguishes "accepted" from
/// "accepted and reduced". The receive path derives every reassembly offset
/// from [`StreamParams::packet_size`] via `gvsp_payload_size`, so believing the
/// request produces a stream that carries packets and completes no frame:
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), where a
/// Vieworks FS3200T on a 16 114-byte link delivered zero frames on every Start
/// and streamed correctly when forced to 1 500. Backlog SR-02.
///
/// The read comes first as well as last. Viva Studio rebuilds the stream on
/// every Acquisition Start, so an unconditional write discards a working
/// configuration once per Start; when the device already holds the size we
/// want, the write is skipped entirely.
///
/// A device that will not answer the read-back is not failed — that would break
/// streaming which works today on a camera whose only fault is refusing a
/// READREG. It warns and keeps the requested value, which is exactly the old
/// behaviour, now visible in the log rather than assumed.
async fn configure_packet_size(
    device: &mut GigeDevice,
    channel: u32,
    requested: u32,
) -> Result<u32, GenicamError> {
    if let Ok(current) = device.get_stream_packet_size(channel).await
        && current == requested
    {
        debug!(
            channel,
            packet_size = requested,
            "camera already holds the requested GVSP packet size; leaving it alone"
        );
        return Ok(requested);
    }

    device
        .set_stream_packet_size(channel, requested)
        .await
        .map_err(|err| GenicamError::transport(err.to_string()))?;

    let effective = match device.get_stream_packet_size(channel).await {
        Ok(effective) => effective,
        Err(err) => {
            warn!(
                channel,
                packet_size = requested,
                error = %err,
                "could not read GevSCPSPacketSize back; assuming the requested size took effect. \
                 If no frame completes, the camera may have clamped it — pass an explicit packet \
                 size of 1500 to find out"
            );
            return Ok(requested);
        }
    };

    if effective == requested {
        return Ok(effective);
    }

    if effective < MIN_PACKET_SIZE {
        return Err(GenicamError::transport(format!(
            "camera reduced the GVSP packet size from {requested} to {effective}, below the \
             {MIN_PACKET_SIZE}-byte minimum; it cannot stream on this link"
        )));
    }

    warn!(
        channel,
        requested,
        effective,
        "camera clamped the GVSP packet size; the receive path will follow the camera. \
         The requested size is what the host interface MTU allows, so the camera is the \
         narrower end of this link"
    );
    Ok(effective)
}

/// Packet size at or below which probing has nothing to discover, and the
/// control size the probe uses to decide whether the device answers at all.
///
/// 1500 is the Ethernet default: a path that cannot carry it is broken in a way
/// no negotiation can rescue.
const PROBE_FLOOR: u32 = 1500;

/// How long to wait for a test packet before calling the size unusable.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Find the largest packet size the network path will actually deliver.
///
/// `GevSCPSPacketSize` reports what the *device* stored, which is not what the
/// *link* will carry. On the Vieworks FS3200T in
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112) the camera
/// declares `Max=16366`, accepts and holds 16114, and streams nothing: the path
/// tops out at a 9216-byte frame, so 9198 is the largest usable size. The
/// reporter found that by bisecting by hand. The specification provides the
/// mechanism to find it automatically — bit 31 asks the device for one test
/// packet, bit 30 forbids fragmenting it — and we used neither.
///
/// **A device that does not implement test packets must not be punished for
/// it.** Walking such a device down to 1500 would be a severe regression for
/// every working jumbo link. So the probe first asks for a test packet at
/// [`PROBE_FLOOR`], a size any functioning path carries: if *that* produces
/// nothing, the device does not answer probes and the requested size is kept
/// unchanged. Only a device that has demonstrably answered is allowed to talk
/// us downwards.
///
/// The probe never increases the size, so an explicitly configured
/// `--packet-size` is still a ceiling.
///
/// **Probing is destructive, so the answer has to be written back.** Asking for
/// a test packet *is* a write to `GevSCPSPacketSize` — there is no separate
/// register — so when the probe returns, the device holds the last size it was
/// *asked about*, which is rarely the size that was chosen. Every path that
/// probed therefore ends by configuring the negotiated value.
///
/// That rewrite also clears the do-not-fragment bit the probe set — except where
/// the device already holds the negotiated size, since `configure_packet_size`
/// skips a redundant write. The exception is the safe one: it is reachable only
/// when a *DF-set* test packet of that size traversed the path a moment earlier,
/// which is the evidence that leaving DF on cannot hurt.
async fn probe_packet_size(
    device: &mut GigeDevice,
    channel: u32,
    socket: &UdpSocket,
    requested: u32,
) -> Result<u32, GenicamError> {
    if requested <= PROBE_FLOOR {
        // Nothing was probed, so nothing was written and there is nothing to
        // put back.
        return Ok(requested);
    }

    let negotiated = probe_path_ceiling(device, channel, socket, requested).await?;

    // The register now holds the last size the probe *tested*, not `negotiated`.
    // Both mismatches are real and neither is visible from here: a device that
    // never answered was left at `PROBE_FLOOR`, and a bisection whose final
    // probe failed was left one byte above its own answer -- 9199 against a
    // negotiated 9198 on the numbers in #112, which is exactly the size that
    // reporter measured as the first failing one. Left uncorrected, the host
    // strides at `negotiated` while the camera sends something else, which is
    // the SR-02 failure mode this probe was built on top of.
    configure_packet_size(device, channel, negotiated).await
}

/// The bisection itself: the largest size that arrives, or `requested` when the
/// device does not answer probes at all.
///
/// Split out from [`probe_packet_size`] so that every early return still passes
/// through the write-back, rather than each one having to remember it.
async fn probe_path_ceiling(
    device: &mut GigeDevice,
    channel: u32,
    socket: &UdpSocket,
    requested: u32,
) -> Result<u32, GenicamError> {
    // Control probe. Establishes that this device answers at all before any
    // negative result is allowed to mean anything.
    if !test_packet_arrives(device, channel, socket, PROBE_FLOOR).await? {
        debug!(
            channel,
            packet_size = requested,
            "device did not answer a {PROBE_FLOOR}-byte test packet; it likely does not \
             implement them, so the requested size stands"
        );
        return Ok(requested);
    }

    if test_packet_arrives(device, channel, socket, requested).await? {
        debug!(
            channel,
            packet_size = requested,
            "path carries the requested GVSP packet size"
        );
        return Ok(requested);
    }

    // Known good at `lo`, known bad at `hi`. Bisect to the byte: the boundary
    // is a frame ceiling, not a round number, and landing one byte below it is
    // the difference between a working jumbo link and 1500.
    let (mut lo, mut hi) = (PROBE_FLOOR, requested);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if test_packet_arrives(device, channel, socket, mid).await? {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    warn!(
        channel,
        requested,
        negotiated = lo,
        "the network path did not carry a {requested}-byte GVSP test packet; negotiated down. \
         Both the host interface and the camera accept the larger size, so the limit is the \
         link between them"
    );
    Ok(lo)
}

/// Ask for one test packet of `size` and report whether it arrived.
async fn test_packet_arrives(
    device: &mut GigeDevice,
    channel: u32,
    socket: &UdpSocket,
    size: u32,
) -> Result<bool, GenicamError> {
    // Discard anything already queued, so a previous probe's late arrival
    // cannot be counted as this one's answer.
    let mut scratch = [0u8; 2048];
    while socket.try_recv_from(&mut scratch).is_ok() {}

    device
        .request_test_packet(channel, size)
        .await
        .map_err(|err| GenicamError::transport(err.to_string()))?;

    let mut buf = vec![0u8; size as usize + 64];
    match timeout(PROBE_TIMEOUT, socket.recv_from(&mut buf)).await {
        Ok(Ok(_)) => Ok(true),
        // A receive error here is the socket's problem, not the path's; treat
        // it as "no answer" rather than failing the whole stream.
        Ok(Err(_)) | Err(_) => Ok(false),
    }
}

/// How long a stream may produce nothing before [`SilenceWatch`] speaks up.
const SILENCE_GRACE: Duration = Duration::from_secs(3);

/// How often the non-Windows receive loop wakes to re-check the watch.
///
/// The Windows reader thread gets the same effect free from its 100 ms socket
/// read timeout.
#[cfg(not(windows))]
const SILENCE_POLL: Duration = Duration::from_millis(500);

/// What a [`SilenceWatch`] has concluded so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Silence {
    /// Nothing to say: still inside the grace period, or already spoken, or the
    /// stream is delivering frames.
    Quiet,
    /// Not one datagram has arrived.
    NoPacket,
    /// Datagrams are arriving and no frame has completed.
    NoFrame,
}

/// Warns once when a stream produces nothing, and names what to check.
///
/// Backlog DX-09. A silent stream reports `frames=0 drops=0 resends=0`, and
/// that line is identical for a firewall block, a packet size the path cannot
/// carry, a control privilege held by another application, and a camera that is
/// simply not triggering. [#70](https://github.com/VitalyVorobyev/viva-genicam/issues/70)'s
/// reporter worked the third of those out unaided;
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)'s needed a
/// custom instrumented build to find the second.
///
/// The distinction the watch adds over "no frames" is whether *datagrams* are
/// arriving, which the receiver already knows and never reported. Nothing
/// arriving is a path or privilege problem; datagrams arriving with no frame
/// completing is a packet-size disagreement, which is exactly SR-02.
struct SilenceWatch {
    since: Instant,
    packet_size: u32,
    saw_packet: bool,
    saw_frame: bool,
    warned: bool,
}

impl SilenceWatch {
    fn new(packet_size: u32) -> Self {
        Self {
            since: Instant::now(),
            packet_size,
            saw_packet: false,
            saw_frame: false,
            warned: false,
        }
    }

    /// A datagram arrived — parsed or not. Malformed still means the path works.
    fn record_packet(&mut self) {
        self.saw_packet = true;
    }

    /// A frame completed, so there is nothing left to diagnose.
    fn record_frame(&mut self) {
        self.saw_frame = true;
    }

    /// Decide what to say at `elapsed`, marking the verdict as spoken.
    ///
    /// Split from [`SilenceWatch::tick`] so the decision is testable without a
    /// clock.
    fn assess(&mut self, elapsed: Duration) -> Silence {
        if self.warned || self.saw_frame || elapsed < SILENCE_GRACE {
            return Silence::Quiet;
        }
        self.warned = true;
        if self.saw_packet {
            Silence::NoFrame
        } else {
            Silence::NoPacket
        }
    }

    /// Emit the warning if one is due. Cheap enough to call on every timeout.
    fn tick(&mut self) {
        let seconds = SILENCE_GRACE.as_secs();
        match self.assess(self.since.elapsed()) {
            Silence::Quiet => {}
            Silence::NoPacket => warn!(
                packet_size = self.packet_size,
                seconds,
                "no GVSP packet has arrived since the stream opened. Check, roughly in order of \
                 how often each is the cause: a host firewall blocking inbound UDP on the stream \
                 port; another application holding control privilege, so AcquisitionStart never \
                 reached the camera; a camera waiting for a trigger; or a network path that \
                 cannot carry packets this large — retry with an explicit packet size of 1500 to \
                 rule that out"
            ),
            Silence::NoFrame => warn!(
                packet_size = self.packet_size,
                payload_stride = gvsp_payload_size(self.packet_size),
                seconds,
                "GVSP packets are arriving but no frame has completed. Reassembly places each \
                 packet at a stride derived from the negotiated packet size, so the usual cause \
                 is the two ends disagreeing about it — retry with an explicit packet size of \
                 1500"
            ),
        }
    }
}

/// How [`StreamBuilder`] chooses `GevSCPSPacketSize` (ADR-0021 / SR-14).
///
/// The variants differ only in where the *starting* size comes from. All three
/// then hand it to the SR-13 path probe, which can lower it and never raise it,
/// so no variant can end up above the size it began with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PacketSizeMode {
    /// Start from the camera's current value; never write a larger one.
    #[default]
    Preserve,
    /// Start from `best_packet_size(nic_mtu)`.
    Auto,
    /// Start from this size — a ceiling, never raised above.
    Explicit(u32),
}

/// Builder for configuring a GVSP stream.
pub struct StreamBuilder<'a> {
    device: &'a mut GigeDevice,
    iface: Option<Iface>,
    dest: Option<StreamDest>,
    rcvbuf_bytes: Option<usize>,
    target_mtu: Option<u32>,
    packet_size_mode: PacketSizeMode,
    packet_delay: Option<u32>,
    channel: u32,
    dst_port: u16,
    probe: bool,
}

impl<'a> StreamBuilder<'a> {
    /// Create a new builder bound to an opened [`GigeDevice`].
    ///
    /// Default packet-size policy is **preserve**: the camera's current
    /// `GevSCPSPacketSize` is used as-is. Call [`StreamBuilder::auto_packet_size`]
    /// to set from the NIC MTU and path-probe, or [`StreamBuilder::packet_size`]
    /// for an explicit ceiling (ADR-0021).
    pub fn new(device: &'a mut GigeDevice) -> Self {
        Self {
            device,
            iface: None,
            dest: None,
            rcvbuf_bytes: None,
            target_mtu: None,
            packet_size_mode: PacketSizeMode::Preserve,
            packet_delay: None,
            channel: 0,
            dst_port: 0,
            // On for every policy, Preserve included: the probe only lowers.
            probe: true,
        }
    }

    /// Whether to path-probe with a GVSP test packet before streaming
    /// (default: on, under every packet-size policy).
    ///
    /// It applies under the default preserve policy too, which is not a
    /// contradiction: the probe never *raises* a size, so it cannot override the
    /// value an operator set. It can only decline to stream at a size the path
    /// demonstrably drops — something no register read can discover, because
    /// both endpoints accept it. Turning it off is what makes preserve literal.
    ///
    /// Turning it off restores the pre-0.4.2 behaviour: whatever
    /// `GevSCPSPacketSize` holds is assumed to reach the host.
    /// That is wrong on any path narrower than both endpoints
    /// ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)), so
    /// the only good reason to disable it is a device that misbehaves when
    /// asked for a test packet — in which case please open an issue, because
    /// the probe is written to tolerate a device that simply ignores it.
    pub fn probe(mut self, enable: bool) -> Self {
        self.probe = enable;
        self
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

    /// Cap the MTU used when computing the GVSP packet size in
    /// [`StreamBuilder::auto_packet_size`] mode.
    ///
    /// The interface's own MTU is still read; this only lowers the request.
    pub fn target_mtu(mut self, mtu: u32) -> Self {
        self.target_mtu = Some(mtu);
        self
    }

    /// Set `GevSCPSPacketSize` from the host NIC MTU, then path-probe (SR-13).
    ///
    /// This is the explicit opt-in that replaces the old always-write-MTU
    /// default (ADR-0021). It is **not** the pre-0.4 `--auto false ⇒ 1500`
    /// behaviour.
    pub fn auto_packet_size(mut self) -> Self {
        self.packet_size_mode = PacketSizeMode::Auto;
        self
    }

    /// Write this GVSP packet size (a ceiling for the path probe).
    pub fn packet_size(mut self, size: u32) -> Self {
        self.packet_size_mode = PacketSizeMode::Explicit(size);
        self
    }

    /// Override the GVSP inter-packet delay.
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

        // ADR-0021: default preserves the camera register. Auto writes the NIC
        // MTU then SR-13-bisects; Explicit writes a caller ceiling. Preserve is
        // never "fall back to 1500" (that was the pre-0.4 auto=false trap).
        let (requested, write_camera) = match self.packet_size_mode {
            PacketSizeMode::Preserve => {
                let current = self
                    .device
                    .get_stream_packet_size(self.channel)
                    .await
                    .map_err(|err| GenicamError::transport(err.to_string()))?;
                debug!(
                    channel = self.channel,
                    packet_size = current,
                    "preserving camera GevSCPSPacketSize; pass auto_packet_size() \
                     or packet_size(n) to negotiate"
                );
                (current, false)
            }
            PacketSizeMode::Auto => (nic::best_packet_size(mtu), true),
            PacketSizeMode::Explicit(size) => (size, true),
        };

        if requested < MIN_PACKET_SIZE {
            return Err(GenicamError::transport(format!(
                "GVSP packet size {requested} is below the {MIN_PACKET_SIZE}-byte minimum; \
                 raise the camera's GevSCPSPacketSize, or override with auto_packet_size() \
                 or packet_size(n)"
            )));
        }
        if requested > MAX_PACKET_SIZE {
            return Err(GenicamError::transport(format!(
                "GVSP packet size {requested} exceeds the {MAX_PACKET_SIZE}-byte maximum an \
                 IPv4 datagram can carry, which is also the widest value GevSCPSPacketSize can \
                 hold"
            )));
        }

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

        // Bind before configuring, so the socket is already listening when the
        // probe below asks the camera to send to it.
        let source = PacketSource::Udp(Self::bind_socket(&dest, &iface, self.rcvbuf_bytes).await?);

        let packet_size = if write_camera {
            configure_packet_size(self.device, self.channel, requested).await?
        } else {
            requested
        };
        // Preserve probes too. The probe only ever *lowers* a size, so it cannot
        // override the value an operator chose -- it can only decline to send at
        // a size the path demonstrably drops, which no register read can reveal
        // (#112). Skipping it under Preserve would leave the reporter's own
        // camera at the 16114 an earlier run wrote, streaming nothing, with the
        // one mechanism that could rescue it turned off. `probe(false)` is the
        // escape for a literal preserve.
        let packet_size = if self.probe {
            let socket = source
                .as_udp_socket()
                .expect("the UDP path always has a socket");
            probe_packet_size(self.device, self.channel, socket, packet_size).await?
        } else {
            packet_size
        };

        // The delay compensates for a burst of many small packets, so it follows
        // the size actually in force -- known only now, after a clamp or a probe.
        // Keying it off the NIC MTU was equivalent while the size was *derived*
        // from that MTU; under Preserve the two come apart, and a jumbo NIC in
        // front of a camera holding 1500 is exactly the case that needs spacing.
        let packet_delay = self.packet_delay.unwrap_or({
            const DELAY_NS: u32 = 2_000;
            if packet_size <= 1500 {
                DELAY_NS / 80
            } else {
                0
            }
        });
        self.device
            .set_stream_packet_delay(self.channel, packet_delay)
            .await
            .map_err(|err| GenicamError::transport(err.to_string()))?;

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
                let bind_ip = IpAddr::V4(
                    iface
                        .ipv4()
                        .ok_or_else(|| GenicamError::transport("interface lacks IPv4 address"))?,
                );
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
/// IPv4 header size used by GigE Vision streams.
const IPV4_HEADER_SIZE: usize = 20;
/// UDP header size used by GigE Vision streams.
const UDP_HEADER_SIZE: usize = 8;
/// Bytes in a GVSP data packet that are not image payload.
const GVSP_PACKET_OVERHEAD: usize = IPV4_HEADER_SIZE + UDP_HEADER_SIZE + GVSP_HEADER_SIZE;

/// The image bytes one GVSP data packet can carry at `packet_size`.
///
/// Also the stride reassembly places packets at, which is why
/// [`SilenceWatch`] reports it: a stream carrying packets that completes no
/// frame is usually a disagreement about this number.
fn gvsp_payload_size(packet_size: u32) -> usize {
    (packet_size as usize).saturating_sub(GVSP_PACKET_OVERHEAD)
}

/// State for a frame being assembled from GVSP packets.
///
/// On Windows the runtime path uses [`WindowsFrameAssembly`] instead, so the
/// fields only this type's completion step reads (`block_id`, `width`, `height`,
/// `pixel_format`, `timestamp`) have no reader there — the completion step lives
/// in `next_frame`, which is `cfg(not(windows))`. The type is still built under
/// `test` so the reassembly unit tests below run on every platform. Unifying the
/// two implementations is backlog API-01; the allow goes away with them.
#[cfg(any(not(windows), test))]
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug)]
struct FrameAssemblyState {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    expected_packets: Option<usize>,
    bitmap: Option<PacketBitmap>,
    received_packet_ids: HashSet<u32>,
    payload: BytesMut,
    packet_payload_size: usize,
    started: Instant,
    trailer_size_y: u32,
}

#[cfg(any(not(windows), test))]
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
            received_packet_ids: HashSet::new(),
            payload: BytesMut::new(),
            packet_payload_size,
            started: Instant::now(),
            trailer_size_y: 0,
        }
    }

    /// Ingest a payload packet. Returns true if this is a new packet.
    fn ingest(&mut self, packet_id: u32, data: &[u8]) -> bool {
        // Packet ID 0 is the leader. GVSP payload packet IDs begin at 1.
        if packet_id == 0 || !self.received_packet_ids.insert(packet_id) {
            return false;
        }

        let pid = packet_id as usize;

        // A resent packet can arrive after the trailer established the expected
        // count, so keep the bitmap in sync in that case.
        if let Some(ref mut bitmap) = self.bitmap
            && !bitmap.set(pid.saturating_sub(1))
        {
            return false; // Duplicate packet.
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

    /// Set expected payload packets from the GVSP trailer packet ID.
    ///
    /// Packet ID 0 belongs to the leader and the trailer immediately follows
    /// the last payload packet, so a complete frame contains every payload ID
    /// in the range `1..trailer_packet_id`.
    fn set_trailer_packet_id(&mut self, trailer_packet_id: u32) {
        if self.expected_packets.is_none() {
            let expected_packets = trailer_packet_id.saturating_sub(1) as usize;
            let mut bitmap = PacketBitmap::new(expected_packets);
            for packet_id in &self.received_packet_ids {
                if *packet_id < trailer_packet_id {
                    bitmap.set(packet_id.saturating_sub(1) as usize);
                }
            }
            self.expected_packets = Some(expected_packets);
            self.bitmap = Some(bitmap);
        }
    }

    fn set_trailer_size_y(&mut self, trailer_size_y: u32) {
        self.trailer_size_y = trailer_size_y;
    }

    /// Check if all packets have been received.
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

#[cfg(windows)]
struct WindowsFrameAssembly {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    next_packet_id: u32,
    payload: BytesMut,
    started: Instant,
    trailer_size_y: u32,
}

#[cfg(windows)]
struct WindowsReceiver {
    frames: tokio::sync::mpsc::Receiver<Result<GenericStreamBlock, GenicamError>>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

#[cfg(windows)]
fn windows_frame_receiver(
    source: PacketSource,
    packet_size: u32,
    stats: StreamStatsAccumulator,
    frame_timeout_ns: Arc<AtomicU64>,
) -> WindowsReceiver {
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let stop = Arc::new(AtomicBool::new(false));

    let PacketSource::Udp(socket) = source;
    let socket = match socket.into_std() {
        Ok(socket) => socket,
        Err(err) => {
            let _ = tx.try_send(Err(GenicamError::transport(format!(
                "socket conversion failed: {err}"
            ))));
            return WindowsReceiver {
                frames: rx,
                stop,
                join: None,
            };
        }
    };
    if let Err(err) = socket.set_nonblocking(false) {
        let _ = tx.try_send(Err(GenicamError::transport(format!(
            "set blocking mode failed: {err}"
        ))));
        return WindowsReceiver {
            frames: rx,
            stop,
            join: None,
        };
    }
    if let Err(err) = socket.set_read_timeout(Some(Duration::from_millis(100))) {
        let _ = tx.try_send(Err(GenicamError::transport(format!(
            "set stream socket read timeout failed: {err}"
        ))));
        return WindowsReceiver {
            frames: rx,
            stop,
            join: None,
        };
    }

    let reader_stop = Arc::clone(&stop);
    let reader = thread::spawn(move || {
        let mut recv_buffer = vec![0u8; (packet_size as usize + 64).max(4096)];
        let mut active: Option<WindowsFrameAssembly> = None;
        let mut silence = SilenceWatch::new(packet_size);

        while !reader_stop.load(Ordering::Acquire) {
            let (len, _) = match socket.recv_from(&mut recv_buffer) {
                Ok(result) => result,
                Err(err)
                    if matches!(
                        err.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    if active.as_ref().is_some_and(|frame| {
                        frame.started.elapsed().as_nanos()
                            > frame_timeout_ns.load(Ordering::Relaxed) as u128
                    }) {
                        active = None;
                        stats.record_drop();
                    }
                    silence.tick();
                    continue;
                }
                Err(err) => {
                    let _ = tx.try_send(Err(GenicamError::transport(format!(
                        "socket receive failed: {err}"
                    ))));
                    break;
                }
            };

            silence.record_packet();

            let packet = match gvsp::parse_packet(&recv_buffer[..len]) {
                Ok(packet) => packet,
                Err(_) => continue,
            };

            match packet {
                GvspPacket::Leader {
                    block_id,
                    width,
                    height,
                    pixel_format,
                    timestamp,
                    ..
                } => {
                    if active.take().is_some() {
                        stats.record_drop();
                    }
                    active = Some(WindowsFrameAssembly {
                        block_id,
                        width,
                        height,
                        pixel_format: PixelFormat::from_code(pixel_format),
                        timestamp,
                        next_packet_id: 1,
                        payload: BytesMut::new(),
                        started: Instant::now(),
                        trailer_size_y: 0,
                    });
                }
                GvspPacket::Payload {
                    block_id,
                    packet_id,
                    data,
                } => {
                    let Some(frame) = active.as_mut() else {
                        continue;
                    };
                    if frame.block_id != block_id || frame.next_packet_id != packet_id {
                        active = None;
                        stats.record_drop();
                        continue;
                    }
                    frame.payload.extend_from_slice(data.as_ref());
                    frame.next_packet_id += 1;
                    stats.record_packet();
                }
                GvspPacket::Trailer {
                    block_id,
                    packet_id,
                    status,
                    chunk_data,
                    size_y,
                    ..
                } => {
                    let Some(mut frame) = active.take() else {
                        continue;
                    };
                    frame.trailer_size_y = size_y;
                    let is_evs = matches!(
                        frame.pixel_format,
                        PixelFormat::EvsEvt30 | PixelFormat::EvsEvt21
                    );
                    let expected_bytes = frame.width as usize
                        * frame.height as usize
                        * frame.pixel_format.bytes_per_pixel().unwrap_or(1);
                    if frame.block_id != block_id
                        || frame.next_packet_id != packet_id
                        || status != 0
                        || (!is_evs && frame.payload.len() < expected_bytes)
                    {
                        stats.record_drop();
                        continue;
                    }
                    if !is_evs {
                        frame.payload.truncate(expected_bytes);
                    }
                    let chunks = if chunk_data.is_empty() {
                        None
                    } else {
                        crate::chunks::parse_chunk_bytes(chunk_data.as_ref()).ok()
                    };
                    let completed = classify_completed_block(
                        frame.payload.freeze(),
                        frame.width,
                        frame.height,
                        frame.pixel_format,
                        chunks,
                        frame.timestamp,
                        None,
                        frame.trailer_size_y,
                    );
                    stats.record_frame(completed.payload().len(), None);
                    silence.record_frame();
                    if tx.try_send(Ok(completed)).is_err() {
                        stats.record_backpressure_drop();
                    }
                }
            }
        }
    });

    WindowsReceiver {
        frames: rx,
        stop,
        join: Some(reader),
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
    #[cfg(not(windows))]
    source: PacketSource,
    stats: StreamStatsAccumulator,
    params: StreamParams,
    config: StreamConfig,
    #[cfg(not(windows))]
    recv_buffer: Vec<u8>,
    #[cfg(not(windows))]
    active: Option<FrameAssemblyState>,
    #[cfg(not(windows))]
    silence: SilenceWatch,
    frame_timeout: Duration,
    #[cfg(windows)]
    frame_timeout_ns: Arc<AtomicU64>,
    #[cfg(windows)]
    frame_rx: tokio::sync::mpsc::Receiver<Result<GenericStreamBlock, GenicamError>>,
    #[cfg(windows)]
    reader_stop: Arc<AtomicBool>,
    #[cfg(windows)]
    reader: Option<thread::JoinHandle<()>>,
    time_sync: Option<TimeSync>,
}

impl FrameStream {
    /// Create a new frame stream from a configured [`Stream`].
    ///
    /// Optionally accepts a [`TimeSync`] for mapping device timestamps to host time.
    pub fn new(stream: Stream, time_sync: Option<TimeSync>) -> Self {
        let (source, stats, params, config) = stream.into_parts();
        let frame_timeout = DEFAULT_FRAME_TIMEOUT;
        #[cfg(not(windows))]
        let buffer_size = (params.packet_size as usize + 64).max(4096);
        #[cfg(windows)]
        let frame_timeout_ns = Arc::new(AtomicU64::new(
            frame_timeout.as_nanos().min(u64::MAX as u128) as u64,
        ));
        #[cfg(windows)]
        let WindowsReceiver {
            frames: frame_rx,
            stop: reader_stop,
            join: reader,
        } = windows_frame_receiver(
            source,
            params.packet_size,
            stats.clone(),
            Arc::clone(&frame_timeout_ns),
        );

        Self {
            #[cfg(not(windows))]
            source,
            stats,
            params,
            config,
            #[cfg(not(windows))]
            recv_buffer: vec![0u8; buffer_size],
            #[cfg(not(windows))]
            active: None,
            #[cfg(not(windows))]
            silence: SilenceWatch::new(params.packet_size),
            frame_timeout,
            #[cfg(windows)]
            frame_timeout_ns,
            #[cfg(windows)]
            frame_rx,
            #[cfg(windows)]
            reader_stop,
            #[cfg(windows)]
            reader,
            time_sync,
        }
    }

    /// Set the frame assembly timeout.
    ///
    /// If a frame is not complete within this duration, it will be dropped
    /// and assembly will move on to the next frame.
    pub fn set_frame_timeout(&mut self, timeout: Duration) {
        self.frame_timeout = timeout;
        #[cfg(windows)]
        self.frame_timeout_ns.store(
            timeout.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
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
        #[cfg(not(windows))]
        {
            self.source.as_udp_socket()
        }
        #[cfg(windows)]
        {
            None
        }
    }

    /// Receive the next complete image frame.
    ///
    /// This backwards-compatible image API also returns an EVS block as a
    /// [`Frame`] carrying the raw bytes and its EVT PFNC code. New callers that
    /// need to distinguish event data should use [`next_block`](Self::next_block).
    pub async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        Ok(self
            .next_block()
            .await?
            .map(GenericStreamBlock::into_legacy_frame))
    }

    /// Receive and classify the next complete GVSP block.
    ///
    /// Unlike [`next_frame`](Self::next_frame), this method distinguishes LUCID
    /// EVT payloads from conventional images without changing the established
    /// frame API.
    pub async fn next_block(&mut self) -> Result<Option<GenericStreamBlock>, GenicamError> {
        #[cfg(windows)]
        {
            return match self.frame_rx.recv().await {
                Some(Ok(mut block)) => {
                    if let Some(timestamp) = block.device_timestamp() {
                        let ts_host = self
                            .time_sync
                            .as_ref()
                            .map(|time_sync| time_sync.to_host_time(timestamp));
                        block.set_host_time(ts_host);
                    }
                    Ok(Some(block))
                }
                Some(Err(err)) => Err(err),
                None => Ok(None),
            };
        }

        #[cfg(not(windows))]
        {
            loop {
                // Check for timeout on active frame assembly.
                if let Some(ref active) = self.active
                    && active.is_expired(self.frame_timeout)
                {
                    let block_id = active.block_id;
                    warn!(
                        block_id,
                        "frame assembly timeout, dropping incomplete frame"
                    );
                    self.stats.record_drop();
                    self.active = None;
                }

                // Receive next packet. The poll deadline exists so the loop
                // keeps turning while nothing arrives: without it neither the
                // DX-09 watch below nor the frame-assembly timeout above can
                // fire on a stream that goes fully silent, because both are
                // only reached when a packet does. The Windows reader thread
                // has always had this, from its 100 ms socket read timeout.
                let raw = match timeout(SILENCE_POLL, self.source.recv(&mut self.recv_buffer)).await
                {
                    Ok(Ok(data)) if data.is_empty() => return Ok(None), // Stream closed.
                    Ok(Ok(data)) => data,
                    Ok(Err(e)) => return Err(e),
                    Err(_elapsed) => {
                        self.silence.tick();
                        continue;
                    }
                };
                self.silence.record_packet();

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
                        if let Some(ref prev) = self.active
                            && prev.block_id != block_id
                        {
                            debug!(
                                old_block = prev.block_id,
                                new_block = block_id,
                                "new leader arrived, dropping incomplete frame"
                            );
                            self.stats.record_drop();
                        }

                        let pixel_format = PixelFormat::from_code(pixel_format);
                        let packet_payload = gvsp_payload_size(self.params.packet_size);

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
                        if let Some(ref mut active) = self.active
                            && active.block_id == block_id
                            && active.ingest(packet_id, data.as_ref())
                        {
                            self.stats.record_packet();
                        }
                    }

                    GvspPacket::Trailer {
                        block_id,
                        packet_id,
                        status,
                        chunk_data,
                        size_y,
                        ..
                    } => {
                        let Some(mut active) = self.active.take() else {
                            continue;
                        };

                        if active.block_id != block_id {
                            // Mismatched trailer, drop and continue.
                            self.stats.record_drop();
                            continue;
                        }

                        if status != 0 {
                            warn!(block_id, status, "trailer reported non-zero status");
                            self.stats.record_drop();
                            continue;
                        }

                        active.set_trailer_size_y(size_y);
                        active.set_trailer_packet_id(packet_id);
                        if !active.is_complete() {
                            warn!(
                                block_id,
                                trailer_packet_id = packet_id,
                                "dropping incomplete frame"
                            );
                            self.stats.record_drop();
                            continue;
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

                        let block = classify_completed_block(
                            payload,
                            active.width,
                            active.height,
                            active.pixel_format,
                            chunks,
                            active.timestamp,
                            ts_host,
                            active.trailer_size_y,
                        );

                        // FrameStream is the sole owner of completed-frame
                        // accounting. Consumers may snapshot this accumulator
                        // through stats_handle(), but must not record the same
                        // frame again.
                        let latency = block
                            .host_time()
                            .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                        self.stats.record_frame(block.payload().len(), latency);
                        self.silence.record_frame();

                        debug!(
                            block_id,
                            width = block.width(),
                            height = block.height(),
                            bytes = block.payload().len(),
                            "frame complete"
                        );

                        return Ok(Some(block));
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
impl Drop for FrameStream {
    fn drop(&mut self) {
        self.reader_stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

// ============================================================================
// USB3 Vision Frame Stream
// ============================================================================

/// Async frame iterator wrapping blocking USB3 Vision bulk reads.
///
/// Internally spawns a blocking reader thread that calls
/// `U3vStream::next_frame()` in a loop and sends converted [`Frame`]
/// values through an mpsc channel. The async consumer reads from the
/// channel via [`next_frame()`](U3vFrameStream::next_frame).
///
/// # Example
///
/// ```rust,ignore
/// let u3v_stream = device.open_stream(payload_size)?;
/// let mut frames = U3vFrameStream::start(u3v_stream);
/// while let Some(frame) = frames.next_frame().await? {
///     println!("{}x{} frame", frame.width, frame.height);
/// }
/// frames.stop();
/// ```
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vFrameStream {
    rx: tokio::sync::mpsc::Receiver<Result<Frame, GenicamError>>,
    stop_tx: tokio::sync::watch::Sender<bool>,
    _reader: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "u3v")]
impl U3vFrameStream {
    /// Start the frame stream from a configured [`U3vStream`].
    ///
    /// The reader thread runs until [`stop()`](Self::stop) is called,
    /// the [`U3vStream`] errors, or the `U3vFrameStream` is dropped.
    ///
    /// [`U3vStream`]: crate::u3v::stream::U3vStream
    pub fn start<T: crate::u3v::usb::UsbTransfer + 'static>(
        mut stream: crate::u3v::stream::U3vStream<T>,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

        let reader = tokio::task::spawn_blocking(move || {
            loop {
                if *stop_rx.borrow() {
                    break;
                }
                match stream.next_frame() {
                    Ok(raw) => {
                        let pixel_format = PixelFormat::from_code(raw.leader.pixel_format);
                        let frame = Frame {
                            payload: raw.payload,
                            width: raw.leader.width,
                            height: raw.leader.height,
                            pixel_format,
                            chunks: None,
                            ts_dev: Some(raw.leader.timestamp),
                            ts_host: None,
                        };
                        if tx.blocking_send(Ok(frame)).is_err() {
                            break; // Receiver dropped.
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(GenicamError::transport(e.to_string())));
                        break;
                    }
                }
            }
        });

        Self {
            rx,
            stop_tx,
            _reader: reader,
        }
    }

    /// Receive the next complete frame.
    ///
    /// Returns `Ok(None)` when the stream ends (reader stopped or errored).
    pub async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        match self.rx.recv().await {
            Some(Ok(frame)) => Ok(Some(frame)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Signal the reader thread to stop.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

// ============================================================================
// USB3 Vision Stream Builder
// ============================================================================

/// Builder for configuring and starting a U3V frame stream.
///
/// Mirrors the GigE [`StreamBuilder`] pattern but for USB3 Vision.
/// Reads image dimensions from the camera features (or accepts explicit
/// overrides) and opens the underlying USB bulk stream.
///
/// # Example
///
/// ```rust,ignore
/// let mut camera = open_u3v_device(device)?;
/// let frames = U3vStreamBuilder::new(&mut camera)
///     .build()?;
/// ```
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vStreamBuilder<'a, T: crate::u3v::usb::UsbTransfer + 'static> {
    camera: &'a mut crate::Camera<crate::U3vRegisterIo<T>>,
    payload_size: Option<u64>,
}

#[cfg(feature = "u3v")]
impl<'a, T: crate::u3v::usb::UsbTransfer + 'static> U3vStreamBuilder<'a, T> {
    /// Create a new builder bound to a camera.
    pub fn new(camera: &'a mut crate::Camera<crate::U3vRegisterIo<T>>) -> Self {
        Self {
            camera,
            payload_size: None,
        }
    }

    /// Override the payload size (bytes per frame).
    ///
    /// When not set, the builder computes it from Width, Height, and
    /// PixelFormat camera features.
    pub fn payload_size(mut self, size: u64) -> Self {
        self.payload_size = Some(size);
        self
    }

    /// Finalise the builder: configure SIRM, start streaming, return
    /// an async [`U3vFrameStream`].
    pub fn build(self) -> Result<U3vFrameStream, GenicamError> {
        let payload_size = match self.payload_size {
            Some(s) => s,
            None => {
                let width: u64 = self
                    .camera
                    .get("Width")?
                    .parse()
                    .map_err(|e| GenicamError::parse(format!("Width: {e}")))?;
                let height: u64 = self
                    .camera
                    .get("Height")?
                    .parse()
                    .map_err(|e| GenicamError::parse(format!("Height: {e}")))?;
                let pf_str = self.camera.get("PixelFormat")?;
                let bpp = PixelFormat::from_name(&pf_str)
                    .bytes_per_pixel()
                    .unwrap_or(1) as u64;
                width * height * bpp
            }
        };

        let mut device = self.camera.transport().lock_device()?;
        let stream = device
            .open_stream(payload_size)
            .map_err(|e| GenicamError::transport(e.to_string()))?;

        Ok(U3vFrameStream::start(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_assembly_state_ingest_tracks_packets() {
        let mut state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);

        // Ingest packets (packet_id 1 and 2 are payload, 0 is leader).
        assert!(state.ingest(1, &[1, 2, 3]));
        assert!(state.ingest(2, &[4, 5, 6]));

        // Duplicate should return false.
        assert!(!state.ingest(1, &[1, 2, 3]));

        state.set_trailer_packet_id(3);
        assert!(state.is_complete());
    }

    #[test]
    fn gvsp_payload_size_excludes_ip_udp_and_gvsp_headers() {
        assert_eq!(gvsp_payload_size(1458), 1422);
        assert_eq!(gvsp_payload_size(9000), 8964);
    }

    #[test]
    fn silence_watch_stays_quiet_inside_the_grace_period() {
        let mut watch = SilenceWatch::new(1500);
        assert_eq!(
            watch.assess(SILENCE_GRACE - Duration::from_millis(1)),
            Silence::Quiet
        );
        // Still unspoken, so the verdict is available once the grace expires.
        assert_eq!(watch.assess(SILENCE_GRACE), Silence::NoPacket);
    }

    #[test]
    fn silence_watch_distinguishes_no_packet_from_no_frame() {
        let mut nothing = SilenceWatch::new(1500);
        assert_eq!(nothing.assess(SILENCE_GRACE), Silence::NoPacket);

        // The distinction is the whole point of DX-09: nothing arriving is a
        // path or privilege problem, while packets arriving with no frame
        // completing is a packet-size disagreement (SR-02).
        let mut packets = SilenceWatch::new(1500);
        packets.record_packet();
        assert_eq!(packets.assess(SILENCE_GRACE), Silence::NoFrame);
    }

    #[test]
    fn silence_watch_speaks_once_and_never_after_a_frame() {
        let mut watch = SilenceWatch::new(1500);
        assert_eq!(watch.assess(SILENCE_GRACE), Silence::NoPacket);
        // A stream that stays silent must not warn on every poll.
        assert_eq!(watch.assess(SILENCE_GRACE * 10), Silence::Quiet);

        let mut healthy = SilenceWatch::new(1500);
        healthy.record_packet();
        healthy.record_frame();
        assert_eq!(healthy.assess(SILENCE_GRACE * 10), Silence::Quiet);
    }

    #[test]
    fn frame_assembly_state_rejects_missing_payload_packets() {
        let mut state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);
        assert!(state.ingest(1, &[1, 2, 3]));

        state.set_trailer_packet_id(3);

        assert!(!state.is_complete());
    }

    #[test]
    fn frame_assembly_state_accepts_out_of_order_payload_packets() {
        let mut state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);
        assert!(state.ingest(2, &[4, 5, 6]));
        assert!(state.ingest(1, &[1, 2, 3]));

        state.set_trailer_packet_id(3);

        assert!(state.is_complete());
    }

    #[test]
    fn frame_assembly_state_timeout() {
        let state = FrameAssemblyState::new(1, 640, 480, PixelFormat::Mono8, 0, 1400);
        assert!(!state.is_expired(Duration::from_secs(10)));
        assert!(state.is_expired(Duration::ZERO));
    }

    #[test]
    fn captured_lucid_evt30_metadata_is_classified_as_evs() {
        let payload = Bytes::from_static(&[0x8e, 0x8a, 0xff, 0xe0]);
        let block = classify_completed_block(
            payload.clone(),
            1,
            64_000,
            PixelFormat::from_code(0x8110_0e30),
            None,
            0x0000_422d_66d4_39c8,
            None,
            824,
        );

        assert_eq!(block.payload(), &payload);
        assert_eq!(block.width(), None);
        assert_eq!(block.height(), None);
        let StreamBlock::Evs(evs) = StreamBlock::from(block) else {
            panic!("EVT 3.0 must not be exposed as an image frame");
        };
        assert_eq!(evs.format, crate::evs::EvsFormat::Evt30);
        assert_eq!(evs.ts_dev, Some(0x0000_422d_66d4_39c8));
        assert_eq!(evs.leader_size_x, 1);
        assert_eq!(evs.leader_size_y, 64_000);
        assert_eq!(evs.trailer_size_y, 824);
        assert!(evs.chunks.is_none());
        assert_eq!(evs.payload, payload);
    }

    #[test]
    fn evt21_is_classified_as_evs() {
        let block = classify_completed_block(
            Bytes::from_static(&[0; 8]),
            1,
            64_000,
            PixelFormat::from_code(0x8140_0e21),
            None,
            7,
            None,
            8,
        );

        let StreamBlock::Evs(evs) = StreamBlock::from(block) else {
            panic!("EVT 2.1 must not be exposed as an image frame");
        };
        assert_eq!(evs.format, crate::evs::EvsFormat::Evt21);
        assert_eq!(evs.payload.len(), 8);
    }

    #[test]
    fn unknown_pfnc_code_remains_an_image() {
        let pixel_format = PixelFormat::from_code(0x0220_ffff);
        let block = classify_completed_block(
            Bytes::from_static(&[1, 2, 3, 4]),
            2,
            2,
            pixel_format,
            None,
            9,
            None,
            2,
        );

        let StreamBlock::Image(frame) = StreamBlock::from(block) else {
            panic!("an unknown PFNC code in an IMAGE leader is still an image");
        };
        assert_eq!(frame.pixel_format, pixel_format);
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(frame.payload.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn legacy_frame_api_preserves_evt_wire_metadata() {
        let payload = Bytes::from_static(&[1, 2, 3]);
        let chunks = [(
            crate::chunks::ChunkKind::Timestamp,
            crate::chunks::ChunkValue::U64(11),
        )]
        .into_iter()
        .collect();
        let block = classify_completed_block(
            payload.clone(),
            1,
            64_000,
            PixelFormat::EvsEvt30,
            Some(chunks),
            11,
            None,
            3,
        );

        let frame = block.into_legacy_frame();
        assert_eq!(frame.payload, payload);
        assert_eq!((frame.width, frame.height), (1, 64_000));
        assert_eq!(frame.pixel_format, PixelFormat::EvsEvt30);
        assert_eq!(frame.ts_dev, Some(11));
        assert_eq!(
            frame
                .chunks
                .as_ref()
                .and_then(|chunks| chunks.get(&crate::chunks::ChunkKind::Timestamp)),
            Some(&crate::chunks::ChunkValue::U64(11))
        );
    }
}
