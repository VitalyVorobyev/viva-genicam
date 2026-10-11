//! GVSP streaming: control-plane setup and the receive engine.
//!
//! [`StreamBuilder`] negotiates the stream with the camera (destination,
//! packet size, delay) and binds the socket. [`FrameStream`] runs the receive
//! engine on it (ADR-0025): one thread per stream reads the socket, assembles
//! blocks — reordered, interleaved, with extended ids — and hands complete
//! frames to the application through a small drop-oldest queue, counting
//! everything it sees in [`StreamStats`].
//!
//! ```rust,ignore
//! let mut frames = FrameStream::new(stream, None);
//! while let Some(frame) = frames.next_frame().await? {
//!     println!("{}x{} frame {}", frame.width, frame.height, frame.block_id);
//! }
//! ```

mod assembly;
#[cfg(fuzzing)]
mod fuzzing;
mod receiver;
mod stats;

#[cfg(fuzzing)]
#[doc(hidden)]
pub use fuzzing::fuzz_assemble;

pub use stats::{StreamStats, StreamStatsAccumulator};

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use tokio::net::UdpSocket;
// Used by the packet-size probe, which runs on the tokio socket before the
// receive thread takes it over.
use tokio::time::timeout;
use tracing::{debug, info, warn};
use viva_pfnc::PixelFormat;

use crate::GenicamError;
use crate::chunks::ChunkMap;
use crate::evs::EvsBlock;
use crate::frame::{Frame, FrameStatus};
use crate::time::TimeSync;
use assembly::AssembledBlock;
use receiver::{Popped, Shared};
use viva_gige::gvcp::{GigeDevice, StreamParams};
use viva_gige::gvsp::StreamConfig;
use viva_gige::nic::{self, DEFAULT_RCVBUF_BYTES, Iface, McOptions};

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

    /// Block id from the transport.
    pub fn block_id(&self) -> u64 {
        match &self.inner {
            StreamBlock::Image(frame) => frame.block_id,
            StreamBlock::Evs(block) => block.block_id,
        }
    }

    /// How the block ended. Only [`FrameStatus::Complete`] blocks are
    /// delivered unless the stream was asked for incomplete ones.
    pub fn status(&self) -> FrameStatus {
        match &self.inner {
            StreamBlock::Image(frame) => frame.status,
            StreamBlock::Evs(block) => block.status,
        }
    }

    fn device_timestamp(&self) -> Option<u64> {
        match &self.inner {
            StreamBlock::Image(frame) => frame.ts_dev,
            StreamBlock::Evs(block) => block.ts_dev,
        }
    }

    fn set_host_time(&mut self, timestamp: Option<SystemTime>) {
        match &mut self.inner {
            StreamBlock::Image(frame) => frame.ts_host = timestamp,
            StreamBlock::Evs(block) => block.ts_host = timestamp,
        }
    }

    fn into_legacy_frame(self) -> Frame {
        match self.inner {
            StreamBlock::Image(frame) => frame,
            StreamBlock::Evs(block) => {
                let mut frame = Frame::new(
                    block.payload,
                    block.leader_size_x,
                    block.leader_size_y,
                    block.format.pixel_format(),
                );
                frame.chunks = block.chunks;
                frame.ts_dev = block.ts_dev;
                frame.ts_host = block.ts_host;
                frame.block_id = block.block_id;
                frame.status = block.status;
                frame
            }
        }
    }
}

/// Turn an assembled block into the application's view of it: event data for
/// the LUCID EVT formats, an image frame otherwise.
fn classify_completed_block(block: AssembledBlock, chunks: Option<ChunkMap>) -> GenericStreamBlock {
    let (width, height, pixel_format, ts_dev) = match block.leader {
        Some(leader) => (
            leader.width,
            leader.height,
            PixelFormat::from_code(leader.pixel_format),
            Some(leader.timestamp),
        ),
        None => (0, 0, PixelFormat::Unknown(0), None),
    };
    let evs = match pixel_format {
        PixelFormat::VendorLucidEvsEvt30 => Some(crate::evs::EvsFormat::Evt30),
        PixelFormat::VendorLucidEvsEvt21 => Some(crate::evs::EvsFormat::Evt21),
        _ => None,
    };
    let inner = match evs {
        Some(format) => StreamBlock::Evs(EvsBlock {
            payload: block.payload,
            format,
            ts_dev,
            leader_size_x: width,
            leader_size_y: height,
            trailer_size_y: block.trailer_size_y,
            chunks,
            ts_host: None,
            block_id: block.block_id,
            status: block.status,
        }),
        None => {
            let mut frame = Frame::new(block.payload, width, height, pixel_format);
            frame.chunks = chunks;
            frame.ts_dev = ts_dev;
            frame.block_id = block.block_id;
            frame.payload_type = block.payload_type;
            frame.status = block.status;
            StreamBlock::Image(frame)
        }
    };
    GenericStreamBlock { inner }
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
/// from [`StreamParams::packet_size`] (the payload stride), so believing the
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
            DEFAULT_STREAM_PORT
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
        let socket = Self::bind_socket(&dest, &iface, self.rcvbuf_bytes).await?;

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
            probe_packet_size(self.device, self.channel, &socket, packet_size).await?
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

        Ok(Stream {
            socket,
            stats: StreamStatsAccumulator::new(),
            params,
            config,
            rcvbuf_bytes: self.rcvbuf_bytes,
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

/// Stream port used when the caller names none.
const DEFAULT_STREAM_PORT: u16 = 0x5FFF;

/// How [`Camera::open_stream`](crate::Camera::open_stream) sets up a GigE
/// stream. The defaults: the interface the operating system routes to the
/// camera, unicast, the camera's own packet size (path-probed, ADR-0021), and
/// a receive buffer the engine sizes from the frame.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    iface: Option<Iface>,
    packet_size: PacketSizeMode,
    multicast: Option<Ipv4Addr>,
    destination_port: Option<u16>,
    rcvbuf_bytes: Option<usize>,
}

impl StreamOptions {
    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Receive on this interface instead of the one routed to the camera.
    pub fn iface(mut self, iface: Iface) -> Self {
        self.iface = Some(iface);
        self
    }

    /// Write this `GevSCPSPacketSize`, a ceiling for the path probe
    /// ([`StreamBuilder::packet_size`]).
    pub fn packet_size(mut self, size: u32) -> Self {
        self.packet_size = PacketSizeMode::Explicit(size);
        self
    }

    /// Set the packet size from the interface MTU, then probe
    /// ([`StreamBuilder::auto_packet_size`]).
    pub fn auto_packet_size(mut self) -> Self {
        self.packet_size = PacketSizeMode::Auto;
        self
    }

    /// Have the camera stream to a multicast group, and join it.
    pub fn multicast(mut self, group: Ipv4Addr) -> Self {
        self.multicast = Some(group);
        self
    }

    /// Receive on this UDP port (default 0x5FFF).
    pub fn destination_port(mut self, port: u16) -> Self {
        self.destination_port = Some(port);
        self
    }

    /// Ask for this `SO_RCVBUF` instead of letting the engine size it from
    /// the frame (`max(4 MiB, 2 × PayloadSize)`).
    pub fn rcvbuf_bytes(mut self, bytes: usize) -> Self {
        self.rcvbuf_bytes = Some(bytes);
        self
    }
}

impl crate::Camera<crate::GigeRegisterIo> {
    /// Open a GVSP stream from this camera, ready for `acquisition_start`.
    ///
    /// The one recipe every consumer uses (API-17): claim control (again, if
    /// it was released), point the stream channel at this host — or at a
    /// multicast group — negotiate the packet size, bind the socket, and start
    /// the receive engine with the camera's `PayloadSize` and time sync. A
    /// camera without a readable `PayloadSize` still streams: buffers are then
    /// sized from each leader.
    // The device lock is held across the builder's awaits: `StreamBuilder`
    // borrows the device, and nothing else may use the control channel while
    // the stream channel is being configured. The lint resolves at the
    // enclosing body, so the allow lives here.
    #[allow(clippy::await_holding_lock)]
    pub fn open_stream(&mut self, options: StreamOptions) -> Result<FrameStream, GenicamError> {
        let port = options.destination_port.unwrap_or(DEFAULT_STREAM_PORT);
        if let Some(group) = options.multicast {
            self.configure_stream_multicast(0, group, port)?;
        }
        let payload_size = read_payload_size(self);
        let time_sync = self.time_sync().clone();
        let transport = self.transport();
        let stream = transport.block_on(async {
            let mut device = transport.lock_device()?;
            device
                .claim_control()
                .await
                .map_err(|err| GenicamError::transport(format!("claim control: {err}")))?;
            let iface = match options.iface {
                Some(iface) => iface,
                None => {
                    let camera_ip = match device.remote_addr().ip() {
                        IpAddr::V4(ip) => ip,
                        IpAddr::V6(_) => {
                            return Err(GenicamError::transport("IPv6 cameras are not supported"));
                        }
                    };
                    Iface::from_remote_ipv4(camera_ip).map_err(|err| {
                        GenicamError::transport(format!(
                            "no local interface routes to {camera_ip}: {err}"
                        ))
                    })?
                }
            };
            let mut builder = StreamBuilder::new(&mut device).iface(iface);
            builder.packet_size_mode = options.packet_size;
            if let Some(bytes) = options.rcvbuf_bytes {
                builder = builder.rcvbuf_bytes(bytes);
            }
            if let Some(group) = options.multicast {
                builder = builder.multicast(Some(group));
            }
            builder = builder.destination_port(port);
            builder.build().await
        })?;
        Ok(FrameStream::start_with(
            stream,
            Some(time_sync),
            payload_size,
        ))
    }
}

/// The camera's `PayloadSize`, or `None` — with a warning unless the camera
/// simply has no such feature — when it cannot be read.
fn read_payload_size<T: viva_genapi::RegisterIo>(camera: &crate::Camera<T>) -> Option<usize> {
    match camera.get("PayloadSize") {
        Ok(text) => {
            let size = text.trim().parse::<u64>().ok().filter(|&size| size > 0);
            if size.is_none() {
                warn!(value = %text, "PayloadSize is not a positive integer; sizing frames from each leader");
            }
            size.and_then(|size| usize::try_from(size).ok())
        }
        Err(GenicamError::GenApi(viva_genapi::GenApiError::NodeNotFound(_))) => None,
        Err(err) => {
            warn!(error = %err, "could not read PayloadSize; sizing frames from each leader");
            None
        }
    }
}

/// Handle returned by [`StreamBuilder`]: the bound socket and the negotiated
/// parameters, ready for [`FrameStream::new`].
pub struct Stream {
    socket: UdpSocket,
    stats: StreamStatsAccumulator,
    params: StreamParams,
    config: StreamConfig,
    /// `SO_RCVBUF` the caller chose, which the engine leaves alone.
    rcvbuf_bytes: Option<usize>,
}

impl Stream {
    /// Access the negotiated stream parameters.
    pub fn params(&self) -> StreamParams {
        self.params
    }

    /// Obtain a clone of the statistics accumulator handle.
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

/// Frames from one GVSP stream, assembled by a receive thread of its own.
///
/// The thread starts when the stream is created and stops, joined within a
/// few milliseconds, when it is dropped. It reads every datagram as it
/// arrives, whether or not anything is waiting for a frame, and keeps up to
/// four finished frames for [`next_block`](Self::next_block) or
/// [`next_block_blocking`](Self::next_block_blocking); when the application
/// falls behind, the oldest is dropped and counted in
/// [`StreamStats::frames_backpressure`].
///
/// Only complete frames are delivered unless
/// [`set_deliver_incomplete`](Self::set_deliver_incomplete) asks otherwise;
/// every block is counted either way.
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
///     println!("Frame {}: {}x{}", frame.block_id, frame.width, frame.height);
/// }
/// ```
pub struct FrameStream {
    shared: Arc<Shared>,
    receiver: Option<JoinHandle<()>>,
    stats: StreamStatsAccumulator,
    params: StreamParams,
    config: Option<StreamConfig>,
    time_sync: Option<TimeSync>,
}

impl FrameStream {
    /// Start receiving on a configured [`Stream`].
    ///
    /// Optionally accepts a [`TimeSync`] for mapping device timestamps to host time.
    pub fn new(stream: Stream, time_sync: Option<TimeSync>) -> Self {
        Self::start_with(stream, time_sync, None)
    }

    /// As [`new`](Self::new), with `PayloadSize` in place before the receive
    /// thread starts.
    fn start_with(
        stream: Stream,
        time_sync: Option<TimeSync>,
        payload_size: Option<usize>,
    ) -> Self {
        let Stream {
            socket,
            stats,
            params,
            config,
            rcvbuf_bytes,
        } = stream;
        let mut frames = Self::idle(stats, params, Some(config), time_sync);
        frames.shared.set_payload_size(payload_size);
        match socket.into_std() {
            Ok(socket) => frames.start(socket, rcvbuf_bytes),
            Err(err) => frames
                .shared
                .queue
                .fail(format!("socket conversion failed: {err}")),
        }
        frames
    }

    /// Receive GVSP on a socket the caller bound, from a camera configured
    /// some other way: another application holds control, or the stream was
    /// set up by hand.
    ///
    /// `packet_size` is the camera's `GevSCPSPacketSize`, which fixes where
    /// each packet's bytes belong. [`config`](Self::config) is `None` for such
    /// a stream.
    pub fn from_socket(
        socket: std::net::UdpSocket,
        packet_size: u32,
    ) -> Result<Self, GenicamError> {
        if assembly::payload_stride(packet_size, viva_gige::gvsp::GVSP_EXTENDED_HEADER_SIZE) == 0 {
            return Err(GenicamError::transport(format!(
                "GVSP packet size {packet_size} leaves no room for data"
            )));
        }
        let local = socket
            .local_addr()
            .map_err(|e| GenicamError::transport(format!("stream socket address: {e}")))?;
        let host = match local.ip() {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
        };
        let params = StreamParams {
            packet_size,
            packet_delay: 0,
            mtu: packet_size,
            host,
            port: local.port(),
        };
        let mut frames = Self::idle(StreamStatsAccumulator::new(), params, None, None);
        frames.start(socket, None);
        Ok(frames)
    }

    fn idle(
        stats: StreamStatsAccumulator,
        params: StreamParams,
        config: Option<StreamConfig>,
        time_sync: Option<TimeSync>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared::new()),
            receiver: None,
            stats,
            params,
            config,
            time_sync,
        }
    }

    fn start(&mut self, socket: std::net::UdpSocket, rcvbuf_bytes: Option<usize>) {
        match receiver::spawn(
            socket,
            self.params.packet_size,
            self.stats.clone(),
            self.shared.clone(),
            rcvbuf_bytes,
        ) {
            Ok(handle) => self.receiver = Some(handle),
            Err(err) => self
                .shared
                .queue
                .fail(format!("could not start the GVSP receive thread: {err}")),
        }
    }

    /// How long a block may go without a packet before it is closed as
    /// incomplete (default 100 ms).
    ///
    /// Counted from the block's **last** packet, not its first: a large frame
    /// on a slow or paced link takes as long as it takes, and only silence
    /// ends it.
    pub fn set_block_timeout(&mut self, timeout: Duration) {
        self.shared.set_block_timeout(timeout);
    }

    /// The camera's `PayloadSize`: the exact size of every frame buffer.
    ///
    /// Without it each buffer is sized from the leader's geometry, which
    /// cannot know about chunk data or vendor padding. Set it before
    /// acquisition starts; blocks already in flight keep their buffers.
    pub fn set_payload_size(&mut self, payload_size: Option<usize>) {
        self.shared
            .set_payload_size(payload_size.filter(|&s| s > 0));
    }

    /// Also deliver frames that did not complete, with their
    /// [`FrameStatus`] saying how (default: off).
    ///
    /// Lost packets' bytes are zero in the payload. Every incomplete frame is
    /// counted in [`StreamStats::frames_incomplete`] either way.
    pub fn set_deliver_incomplete(&mut self, deliver: bool) {
        self.shared.set_deliver_incomplete(deliver);
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

    /// The stream configuration, or `None` for a stream made with
    /// [`from_socket`](Self::from_socket).
    pub fn config(&self) -> Option<&StreamConfig> {
        self.config.as_ref()
    }

    /// Receive the next image frame.
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

    /// Receive and classify the next GVSP block.
    ///
    /// `Ok(None)` means the stream has ended. Cancel-safe: dropping the
    /// future, as `tokio::select!` does, loses no block.
    pub async fn next_block(&mut self) -> Result<Option<GenericStreamBlock>, GenicamError> {
        let popped = self.shared.queue.pop().await;
        self.take(popped)
    }

    /// Wait up to `timeout` for the next block on the calling thread.
    ///
    /// For callers without an async runtime. `Ok(None)` means the stream has
    /// ended; nothing within `timeout` is [`GenicamError::Timeout`].
    pub fn next_block_blocking(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GenericStreamBlock>, GenicamError> {
        match self.shared.queue.pop_blocking(timeout) {
            Popped::Empty => Err(GenicamError::Timeout(timeout)),
            popped => self.take(popped),
        }
    }

    /// [`next_frame`](Self::next_frame) on the calling thread, waiting up to
    /// `timeout`: `Ok(None)` when the stream has ended,
    /// [`GenicamError::Timeout`] when nothing arrived.
    pub fn next_frame_blocking(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<Frame>, GenicamError> {
        Ok(self
            .next_block_blocking(timeout)?
            .map(GenericStreamBlock::into_legacy_frame))
    }

    /// Account for a block leaving the queue and give it its host time.
    fn take(
        &mut self,
        popped: Popped<GenericStreamBlock>,
    ) -> Result<Option<GenericStreamBlock>, GenicamError> {
        match popped {
            Popped::Item(mut block) => {
                let ts_host = block.device_timestamp().and_then(|timestamp| {
                    self.time_sync
                        .as_ref()
                        .and_then(|time_sync| time_sync.to_host_time(timestamp))
                });
                block.set_host_time(ts_host);
                if block.status().is_complete() {
                    let latency = ts_host.and_then(|ts| SystemTime::now().duration_since(ts).ok());
                    self.stats.record_frame(block.payload().len(), latency);
                } else {
                    self.stats.record_incomplete_frame();
                }
                Ok(Some(block))
            }
            Popped::Failed(message) => Err(GenicamError::transport(message)),
            Popped::Finished | Popped::Empty => Ok(None),
        }
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.shared
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(receiver) = self.receiver.take() {
            let _ = receiver.join();
        }
    }
}

// ============================================================================
// USB3 Vision Frame Stream
// ============================================================================

/// Async frame iterator wrapping blocking USB3 Vision bulk reads.
///
/// Internally spawns a blocking reader thread that reads blocks from a
/// [`U3vStream`] in a loop and sends converted [`Frame`] values through an
/// mpsc channel. The async consumer reads from the channel via
/// [`next_frame()`](U3vFrameStream::next_frame).
///
/// A stream from [`U3vStreamBuilder`] also clears the device's StreamEnable
/// when it ends: [`shutdown`](Self::shutdown) once the reader has stopped, or
/// a drop.
///
/// [`U3vStream`]: crate::u3v::stream::U3vStream
///
/// # Example
///
/// ```rust,ignore
/// let mut frames = U3vStreamBuilder::new(&mut camera).build()?;
/// camera.acquisition_start()?;
/// while let Some(frame) = frames.next_frame().await? {
///     println!("{}x{} frame", frame.width, frame.height);
/// }
/// frames.shutdown().await;
/// ```
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vFrameStream {
    rx: tokio::sync::mpsc::Receiver<Result<Frame, GenicamError>>,
    stop_tx: tokio::sync::watch::Sender<bool>,
    reader: Option<tokio::task::JoinHandle<()>>,
    /// Clears StreamEnable; see [`DisableStream`].
    disable: Option<DisableStream>,
    /// Blocks dropped because their trailer reported an error.
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// Clears a device's StreamEnable. Called with `true` it waits for the
/// device lock; with `false` it gives up if someone holds the lock, and
/// returns whether it ran.
#[cfg(feature = "u3v")]
type DisableStream = std::sync::Arc<dyn Fn(bool) -> bool + Send + Sync>;

/// Whether a USB3 Vision trailer status is an error: the severity bit (15)
/// of a GenCP-style status code (GenCP 1.2 §4.3.2.1). A warning or info
/// status leaves the block valid.
#[cfg(feature = "u3v")]
fn trailer_reports_an_error(status: u16) -> bool {
    status & 0x8000 != 0
}

/// How long the U3V reader waits for a frame to start before checking whether
/// it was told to stop.
#[cfg(feature = "u3v")]
const U3V_READ_POLL: Duration = Duration::from_millis(250);

#[cfg(feature = "u3v")]
impl U3vFrameStream {
    /// Start the frame stream from a configured [`U3vStream`].
    ///
    /// The reader thread runs until [`stop()`](Self::stop) is called,
    /// the [`U3vStream`] errors, or the `U3vFrameStream` is dropped. Nothing
    /// clears the device's StreamEnable; [`U3vStreamBuilder`] starts streams
    /// that do.
    ///
    /// [`U3vStream`]: crate::u3v::stream::U3vStream
    pub fn start<T: crate::u3v::usb::UsbTransfer + 'static>(
        stream: crate::u3v::stream::U3vStream<T>,
    ) -> Self {
        Self::start_inner(stream, None)
    }

    fn start_inner<T: crate::u3v::usb::UsbTransfer + 'static>(
        mut stream: crate::u3v::stream::U3vStream<T>,
        disable: Option<DisableStream>,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let reader_dropped = std::sync::Arc::clone(&dropped);

        let reader = tokio::task::spawn_blocking(move || {
            loop {
                // Told to stop, or dropped without being told: nobody is
                // listening, and an idle reader would otherwise poll forever.
                if *stop_rx.borrow() || tx.is_closed() {
                    break;
                }
                // No frame within the poll is a stream waiting, as in trigger
                // mode, not a failure; it is also when the reader can stop.
                match stream.next_frame_within(U3V_READ_POLL) {
                    Ok(None) => continue,
                    // A block the device marks failed is not an image to
                    // hand on, short or not (Codex review of #184).
                    Ok(Some(raw)) if trailer_reports_an_error(raw.trailer.status) => {
                        reader_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tracing::debug!(
                            block_id = raw.trailer.block_id,
                            status = format_args!("{:#06x}", raw.trailer.status),
                            "dropping a block the device marked failed"
                        );
                    }
                    Ok(Some(raw)) => {
                        let pixel_format = PixelFormat::from_code(raw.leader.pixel_format);
                        let mut frame = Frame::new(
                            raw.payload,
                            raw.leader.width,
                            raw.leader.height,
                            pixel_format,
                        );
                        frame.ts_dev = Some(raw.leader.timestamp);
                        frame.block_id = raw.leader.block_id;
                        frame.payload_type = raw.leader.payload_type;
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
            reader: Some(reader),
            disable,
            dropped,
        }
    }

    /// Blocks dropped since the stream started because their trailer
    /// reported an error.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
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

    /// Signal the reader thread to stop. It stops within one read poll, but
    /// this does not wait for it; [`shutdown`](Self::shutdown) does.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    /// Stop the reader thread, wait for it to finish, then clear the device's
    /// StreamEnable: the endpoint and device are free once this returns, for
    /// another stream or for closing the camera (Codex reviews of #179 and
    /// #181).
    pub async fn shutdown(mut self) {
        // A reader blocked handing over a frame nobody takes wakes up here.
        self.rx.close();
        let _ = self.stop_tx.send(true);
        if let Some(reader) = self.reader.take() {
            let _ = reader.await;
        }
        if let Some(disable) = self.disable.take() {
            let _ = tokio::task::spawn_blocking(move || disable(true)).await;
        }
    }
}

#[cfg(feature = "u3v")]
impl Drop for U3vFrameStream {
    /// A stream dropped without [`shutdown`](Self::shutdown) still stops the
    /// device's stream. If the device is busy, a thread of its own waits for
    /// it: the dropping thread must not block on a lock it may hold, and
    /// giving up left StreamEnable set (Codex review of #184).
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        if let Some(disable) = self.disable.take()
            && !disable(false)
        {
            std::thread::spawn(move || {
                disable(true);
            });
        }
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
/// Whether reading `PayloadSize` failed because the camera has no such
/// feature, the one failure a computed size may stand in for.
#[cfg(feature = "u3v")]
fn payload_size_is_absent(error: &GenicamError) -> bool {
    matches!(
        error,
        GenicamError::GenApi(viva_genapi::GenApiError::NodeNotFound(_))
    )
}

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
    /// When not set, the builder reads the camera's `PayloadSize`, or
    /// computes it from Width, Height and a whole-byte PixelFormat.
    pub fn payload_size(mut self, size: u64) -> Self {
        self.payload_size = Some(size);
        self
    }

    /// Bytes in one frame. The camera's `PayloadSize` is the authority: it
    /// knows its own packing and padding. Without it, width × height × the
    /// format's whole bytes a pixel; a packed format such as `Mono12Packed`
    /// has none, and guessing one byte made the reader take the rest of the
    /// image for the trailer and fail (Codex review of #179).
    fn frame_size(&self) -> Result<u64, GenicamError> {
        match self.camera.get("PayloadSize") {
            Ok(text) => {
                if let Some(size) = text.parse::<u64>().ok().filter(|&size| size > 0) {
                    return Ok(size);
                }
            }
            // Only a camera without the feature falls back. One that has it
            // but cannot answer — I/O, access, availability — must not get a
            // computed size that misses its padding or chunk data (Codex
            // review of #180).
            Err(error) if payload_size_is_absent(&error) => {}
            Err(error) => return Err(error),
        }
        let dimension = |name: &str| -> Result<u64, GenicamError> {
            self.camera
                .get(name)?
                .parse()
                .map_err(|e| GenicamError::parse(format!("{name}: {e}")))
        };
        let pixel_format = self.camera.get("PixelFormat")?;
        let bytes = PixelFormat::from_name(&pixel_format)
            .bytes_per_pixel()
            .ok_or_else(|| {
                GenicamError::parse(format!(
                    "cannot size a {pixel_format} frame: the camera has no PayloadSize and the \
                     format has no whole-byte pixel; pass U3vStreamBuilder::payload_size"
                ))
            })?;
        Ok(dimension("Width")? * dimension("Height")? * bytes as u64)
    }

    /// Finalise the builder: configure the SIRM, set StreamEnable, and return
    /// an async [`U3vFrameStream`] that clears it again when it ends.
    ///
    /// The device's SIRM Required Payload Size sizes the frames; the
    /// builder's payload size — set, or read from the camera — is used only
    /// when the device leaves that register at 0.
    pub fn build(self) -> Result<U3vFrameStream, GenicamError> {
        let payload_hint = match self.payload_size {
            Some(s) => s,
            None => self.frame_size()?,
        };

        let device = self.camera.transport().shared_device();
        let stream = device
            .lock()
            .map_err(|_| GenicamError::transport("u3v device mutex poisoned"))?
            .open_stream(payload_hint)
            .map_err(|e| GenicamError::transport(e.to_string()))?;

        let disable: DisableStream = std::sync::Arc::new(move |wait: bool| {
            let guard = if wait {
                device.lock().ok()
            } else {
                device.try_lock().ok()
            };
            let Some(mut device) = guard else {
                return false;
            };
            if let Err(error) = device.stop_stream() {
                tracing::debug!(%error, "clearing StreamEnable");
            }
            true
        });
        Ok(U3vFrameStream::start_inner(stream, Some(disable)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly::LeaderInfo;

    /// A complete assembled block with a leader, as the engine hands it over.
    fn assembled(
        payload: Bytes,
        width: u32,
        height: u32,
        pixel_format: u32,
        timestamp: u64,
        trailer_size_y: u32,
    ) -> AssembledBlock {
        AssembledBlock {
            block_id: 1,
            status: FrameStatus::Complete,
            leader: Some(LeaderInfo {
                timestamp,
                pixel_format,
                width,
                height,
            }),
            payload_type: 0x0001,
            payload,
            trailer_size_y,
            chunk_data: None,
        }
    }

    /// Only a missing `PayloadSize` may be replaced by a computed size (Codex
    /// review of #180).
    #[cfg(feature = "u3v")]
    #[test]
    fn only_a_missing_payload_size_falls_back() {
        use viva_genapi::GenApiError;
        let missing = GenicamError::GenApi(GenApiError::NodeNotFound("PayloadSize".into()));
        let refused = GenicamError::GenApi(GenApiError::Access("PayloadSize".into()));
        let failed = GenicamError::Transport("bulk read".into());
        assert!(payload_size_is_absent(&missing));
        assert!(!payload_size_is_absent(&refused));
        assert!(!payload_size_is_absent(&failed));
    }

    #[test]
    fn captured_lucid_evt30_metadata_is_classified_as_evs() {
        let payload = Bytes::from_static(&[0x8e, 0x8a, 0xff, 0xe0]);
        let block = classify_completed_block(
            assembled(
                payload.clone(),
                1,
                64_000,
                0x8110_0e30,
                0x0000_422d_66d4_39c8,
                824,
            ),
            None,
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
            assembled(Bytes::from_static(&[0; 8]), 1, 64_000, 0x8140_0e21, 7, 8),
            None,
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
            assembled(
                Bytes::from_static(&[1, 2, 3, 4]),
                2,
                2,
                pixel_format.code(),
                9,
                2,
            ),
            None,
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
            assembled(
                payload.clone(),
                1,
                64_000,
                PixelFormat::VendorLucidEvsEvt30.code(),
                11,
                3,
            ),
            Some(chunks),
        );

        let frame = block.into_legacy_frame();
        assert_eq!(frame.payload, payload);
        assert_eq!((frame.width, frame.height), (1, 64_000));
        assert_eq!(frame.pixel_format, PixelFormat::VendorLucidEvsEvt30);
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
