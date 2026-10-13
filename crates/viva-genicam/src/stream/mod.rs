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
mod resend;
mod stats;
#[cfg(feature = "u3v")]
mod u3v;

#[cfg(fuzzing)]
#[doc(hidden)]
pub use fuzzing::fuzz_assemble;

pub use stats::{StreamStats, StreamStatsAccumulator};
#[cfg(feature = "u3v")]
pub use u3v::{U3vFrameStream, U3vStreamBuilder};

/// The queue a USB3 Vision event reader hands events through.
#[cfg(feature = "u3v")]
pub(crate) type EventQueue = receiver::DeliveryQueue<crate::events::Event, viva_u3v::U3vError>;
/// What a pop from an [`EventQueue`] found.
#[cfg(feature = "u3v")]
pub(crate) type EventPopped = receiver::Popped<crate::events::Event, viva_u3v::U3vError>;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
use crate::chunks::ChunkError;
use crate::error::{gige, network};
use crate::evs::EvsBlock;
use crate::frame::{Frame, FrameStatus};
use assembly::AssembledBlock;
use receiver::{Popped, Shared};
use viva_gige::GigeError;
use viva_gige::StreamConfig;
use viva_gige::TimeSync;
use viva_gige::{DEFAULT_RCVBUF_BYTES, Iface, McOptions};
use viva_gige::{GigeDevice, StreamParams};

pub use viva_gige::StreamDest;

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
///
/// A complete block whose payload type carries chunks is split into its image
/// and its chunks (TC-06); a chunk layout that does not parse is returned
/// alongside, so the receive thread can say so, and the block keeps its bytes.
/// An incomplete block is not split: its missing packets are zeros, and zeros
/// read as tags would invent chunks.
fn classify_completed_block(block: AssembledBlock) -> (GenericStreamBlock, Option<ChunkError>) {
    let raw_payload = block.payload.clone();
    let (payload, chunks) = if block.status.is_complete() {
        crate::chunks::split(block.payload, block.payload_type)
    } else {
        (block.payload, None)
    };
    let (chunks, chunk_error) = match chunks {
        Some(Ok(chunks)) => (Some(chunks), None),
        Some(Err(err)) => (None, Some(err)),
        None => (None, None),
    };
    let (width, height, pixel_format, ts_dev, padding_x) = match block.leader {
        Some(leader) => (
            leader.width,
            leader.height,
            PixelFormat::from_code(leader.pixel_format),
            Some(leader.timestamp),
            leader.padding_x,
        ),
        None => (0, 0, PixelFormat::Unknown(0), None, 0),
    };
    let evs = match pixel_format {
        PixelFormat::VendorLucidEvsEvt30 => Some(crate::evs::EvsFormat::Evt30),
        PixelFormat::VendorLucidEvsEvt21 => Some(crate::evs::EvsFormat::Evt21),
        _ => None,
    };
    let inner = match evs {
        Some(format) => StreamBlock::Evs(EvsBlock {
            payload,
            format,
            ts_dev,
            leader_size_x: width,
            leader_size_y: height,
            trailer_size_y: block.trailer_size_y.unwrap_or(0),
            chunks,
            ts_host: None,
            block_id: block.block_id,
            status: block.status,
        }),
        None => {
            let mut frame = Frame::new(payload, width, height, pixel_format);
            frame.raw_payload = raw_payload;
            frame.padding_x = padding_x;
            frame.chunks = chunks;
            frame.ts_dev = ts_dev;
            frame.block_id = block.block_id;
            frame.payload_type = block.payload_type;
            frame.status = block.status;
            // A chunk-data trailer's Size Y is the payload length, not lines.
            frame.delivered_height = block
                .trailer_size_y
                .filter(|_| viva_gige::is_image_payload(block.payload_type));
            frame.expected_size = block.expected_size;
            StreamBlock::Image(frame)
        }
    };
    (GenericStreamBlock { inner }, chunk_error)
}

/// Smallest GVSP packet size we will configure: the IPv4 minimum reassembly
/// buffer.
const MIN_PACKET_SIZE: u32 = 576;

/// Largest GVSP packet size we will configure.
///
/// Both bounds bite at once: an IPv4 datagram cannot exceed 65 535 bytes, and
/// `GevSCPSPacketSize` holds the size in 16 bits.
const MAX_PACKET_SIZE: u32 = viva_gige::consts::STREAM_PACKET_SIZE_MASK;

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
        .map_err(gige("write GevSCPSPacketSize"))?;

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
        warn!(
            requested,
            effective, "the camera reduced the GVSP packet size below the minimum"
        );
        return Err(GenicamError::PacketTooSmall {
            size: effective,
            min: MIN_PACKET_SIZE,
        });
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
        .map_err(gige("request a test packet"))?;

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

/// Whether a stream asks the camera to send lost packets again (SR-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ResendPolicy {
    /// Ask, if the camera's GVCP Capability register (`0x0934`) says it
    /// supports `PACKETRESEND_CMD`. A multicast stream never asks.
    #[default]
    Auto,
    /// Never ask.
    Off,
}

/// Builder for configuring a GVSP stream.
pub struct StreamBuilder<'a> {
    device: &'a mut GigeDevice,
    iface: Option<Iface>,
    dest: Option<StreamDest>,
    rcvbuf_bytes: Option<usize>,
    target_mtu: Option<u32>,
    packet_size_mode: PacketSizeMode,
    packet_delay: Option<Duration>,
    resend: ResendPolicy,
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
            resend: ResendPolicy::Auto,
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

    /// Have the camera wait `delay` between stream packets: `GevSCPD`,
    /// written in timestamp ticks at the frequency the camera reports
    /// (`0x093C`/`0x0940`), rounded to the nearest tick.
    ///
    /// Without it `GevSCPD` is left as it is. A camera that reports no tick
    /// frequency cannot be given a delay in time, and the build fails rather
    /// than guess a unit.
    pub fn packet_delay(mut self, delay: Duration) -> Self {
        self.packet_delay = Some(delay);
        self
    }

    /// Whether to ask the camera for lost packets (default
    /// [`ResendPolicy::Auto`]).
    pub fn resend(mut self, policy: ResendPolicy) -> Self {
        self.resend = policy;
        self
    }

    /// Receive on this UDP port, for a firewall that admits only it.
    ///
    /// By default the operating system picks a free port; either way the
    /// port is written to the camera's `GevSCPHostPort`. The port is the
    /// stream's alone: it is bound without `SO_REUSEPORT`, so a second
    /// stream asking for the same one fails to open.
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
    ///
    /// The build fails for a channel at or beyond the camera's
    /// `GevNumberOfStreamChannels` (`0x0904`), before anything is written.
    pub fn channel(mut self, channel: u32) -> Self {
        self.channel = channel;
        self
    }

    /// Finalise the builder and return a configured [`Stream`].
    ///
    /// In order: check the channel exists and that a requested packet delay
    /// can be converted, bind the socket (port 0 unless one was asked for),
    /// point the stream channel at it, negotiate the packet size, write
    /// `GevSCPD` if asked, and read whether the camera supports resend.
    pub async fn build(self) -> Result<Stream, GenicamError> {
        let iface = self.iface.ok_or(GenicamError::NoInterface)?;
        let host_ip = iface
            .ipv4()
            .ok_or_else(|| GenicamError::InterfaceWithoutIpv4(iface.name().to_string()))?;
        let camera = self.device.remote_addr();
        let IpAddr::V4(camera_ip) = camera.ip() else {
            return Err(GigeError::NotIpv4("camera").into());
        };

        // Refusals first, so a refused stream has written nothing.
        check_stream_channel(self.device, self.channel).await?;
        let packet_delay = match self.packet_delay {
            Some(delay) => Some(packet_delay_ticks(self.device, delay).await?),
            None => None,
        };

        let mut dest = self.dest.unwrap_or(StreamDest::Unicast {
            dst_ip: host_ip,
            dst_port: self.dst_port,
        });

        let iface_mtu = viva_gige::mtu(&iface).bytes();
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
                    .map_err(gige("read GevSCPSPacketSize"))?;
                debug!(
                    channel = self.channel,
                    packet_size = current,
                    "preserving camera GevSCPSPacketSize; pass auto_packet_size() \
                     or packet_size(n) to negotiate"
                );
                (current, false)
            }
            PacketSizeMode::Auto => (viva_gige::best_packet_size(mtu), true),
            PacketSizeMode::Explicit(size) => (size, true),
        };

        if requested < MIN_PACKET_SIZE {
            return Err(GenicamError::PacketTooSmall {
                size: requested,
                min: MIN_PACKET_SIZE,
            });
        }
        if requested > MAX_PACKET_SIZE {
            return Err(GenicamError::PacketTooLarge {
                size: requested,
                max: MAX_PACKET_SIZE,
            });
        }

        // Bind before configuring, so the socket is already listening when the
        // probe below asks the camera to send to it — and so the port the
        // operating system chose is known (SR-03).
        let socket = Self::bind_socket(&dest, &iface, self.rcvbuf_bytes).await?;
        let port = socket
            .local_addr()
            .map_err(network("read the stream socket's address"))?
            .port();
        match &mut dest {
            StreamDest::Unicast { dst_port, .. } => *dst_port = port,
            StreamDest::Multicast { port: p, .. } => *p = port,
        }
        info!(
            addr = %dest.addr(),
            port,
            channel = self.channel,
            multicast = dest.is_multicast(),
            "configuring the stream channel"
        );
        self.device
            .set_stream_destination(self.channel, dest.addr(), port)
            .await
            .map_err(gige("write GevSCDA/GevSCPHostPort"))?;

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

        // SR-19: `GevSCPD` is the operator's unless the caller asked.
        if let Some(ticks) = packet_delay {
            self.device
                .set_stream_packet_delay(self.channel, ticks)
                .await
                .map_err(gige("write GevSCPD"))?;
        }

        let resend = match self.resend {
            ResendPolicy::Off => false,
            ResendPolicy::Auto if dest.is_multicast() => false,
            ResendPolicy::Auto => camera_supports_resend(self.device).await,
        };
        info!(
            channel = self.channel,
            resend,
            "packet resend {}",
            if resend { "on" } else { "off" }
        );

        let params = StreamParams {
            packet_size,
            packet_delay,
            mtu,
            host: dest.addr(),
            port,
            channel: self.channel,
            camera: Some(camera),
        };

        let config = StreamConfig {
            dest,
            iface: iface.clone(),
            packet_size: Some(packet_size),
            packet_delay,
            source_filter: Some(camera_ip),
        };

        Ok(Stream {
            socket,
            stats: StreamStatsAccumulator::new(),
            params,
            config,
            rcvbuf_bytes: self.rcvbuf_bytes,
            resend,
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
                let bind_ip =
                    IpAddr::V4(iface.ipv4().ok_or_else(|| {
                        GenicamError::InterfaceWithoutIpv4(iface.name().to_string())
                    })?);
                viva_gige::bind_udp(bind_ip, *dst_port, Some(iface.clone()), rcvbuf_bytes)
                    .await
                    .map_err(network("bind the stream socket"))
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
                viva_gige::bind_multicast(iface, *group, *port, &opts)
                    .await
                    .map_err(network("join the multicast group"))
            }
        }
    }
}

/// SR-21: refuse a stream channel the camera does not have.
///
/// A count of 0, or one that cannot be read, says nothing usable: channel 0,
/// which every streaming device has, goes ahead with a warning, and any other
/// is refused.
async fn check_stream_channel(device: &mut GigeDevice, channel: u32) -> Result<(), GenicamError> {
    let count = match device.stream_channel_count().await {
        Ok(0) => {
            warn!("the camera reports 0 stream channels (GevNumberOfStreamChannels)");
            None
        }
        Ok(count) => Some(count),
        Err(err) => {
            warn!(error = %err, "could not read GevNumberOfStreamChannels");
            None
        }
    };
    match count {
        Some(count) if channel >= count => Err(GenicamError::StreamChannel {
            channel,
            count: Some(count),
        }),
        None if channel > 0 => Err(GenicamError::StreamChannel {
            channel,
            count: None,
        }),
        _ => Ok(()),
    }
}

/// SR-19: `delay` in the camera's timestamp ticks, the unit of `GevSCPD`.
async fn packet_delay_ticks(device: &mut GigeDevice, delay: Duration) -> Result<u32, GenicamError> {
    if delay.is_zero() {
        return Ok(0);
    }
    let hz = device
        .timestamp_tick_frequency()
        .await
        .map_err(gige("read the timestamp tick frequency, GevSCPD's unit"))?;
    delay_in_ticks(delay, hz)
}

/// `delay` at `hz` ticks a second, rounded to the nearest tick and never 0
/// for a delay that is not.
fn delay_in_ticks(delay: Duration, hz: u64) -> Result<u32, GenicamError> {
    if hz == 0 {
        return Err(GenicamError::PacketDelay {
            delay,
            problem: "the camera reports a timestamp tick frequency of 0, so GevSCPD has no unit",
        });
    }
    let ticks = (delay.as_nanos() * u128::from(hz) + 500_000_000) / 1_000_000_000;
    let ticks = if ticks == 0 && !delay.is_zero() {
        1
    } else {
        ticks
    };
    u32::try_from(ticks).map_err(|_| GenicamError::PacketDelay {
        delay,
        problem: "it is more ticks of the camera's clock than GevSCPD holds",
    })
}

/// Whether the GVCP Capability register says the camera answers
/// `PACKETRESEND_CMD`. Unreadable counts as no.
async fn camera_supports_resend(device: &mut GigeDevice) -> bool {
    match device.gvcp_capability().await {
        Ok(caps) => caps.packet_resend(),
        Err(err) => {
            debug!(error = %err, "could not read GVCP Capability; packet resend stays off");
            false
        }
    }
}

/// How [`Camera::open_stream`](crate::Camera::open_stream) sets up a GigE
/// stream. The defaults: the interface the operating system routes to the
/// camera, unicast to a port the operating system picks, the camera's own
/// packet size (path-probed, ADR-0021) and packet delay, packet resend when
/// the camera supports it, and a receive buffer the engine sizes from the
/// frame.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    iface: Option<Iface>,
    packet_size: PacketSizeMode,
    multicast: Option<Ipv4Addr>,
    destination_port: Option<u16>,
    rcvbuf_bytes: Option<usize>,
    packet_delay: Option<Duration>,
    resend: ResendPolicy,
    monitor: bool,
    payload_size: Option<usize>,
}

impl StreamOptions {
    /// Watch a multicast stream another application configured: claim
    /// nothing, write nothing, read the group, port and packet size from the
    /// camera's stream channel registers, join the group and receive. Never
    /// asks for lost packets.
    ///
    /// The camera must already stream to a multicast group; one streaming
    /// unicast to another host cannot be watched. Works on a camera opened
    /// with [`Access::Monitor`](crate::Access::Monitor) and on one this
    /// application controls.
    pub fn monitor() -> Self {
        Self {
            monitor: true,
            ..Self::default()
        }
    }

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

    /// Receive on this UDP port, for a firewall that admits only it
    /// ([`StreamBuilder::destination_port`]). By default the operating system
    /// picks a free one.
    pub fn destination_port(mut self, port: u16) -> Self {
        self.destination_port = Some(port);
        self
    }

    /// Have the camera wait `delay` between packets
    /// ([`StreamBuilder::packet_delay`]). By default `GevSCPD` is left alone.
    pub fn packet_delay(mut self, delay: Duration) -> Self {
        self.packet_delay = Some(delay);
        self
    }

    /// Whether to ask the camera for lost packets (default
    /// [`ResendPolicy::Auto`]).
    pub fn resend(mut self, policy: ResendPolicy) -> Self {
        self.resend = policy;
        self
    }

    /// Ask for this `SO_RCVBUF` instead of letting the engine size it from
    /// the frame (`max(4 MiB, 2 × PayloadSize)`).
    pub fn rcvbuf_bytes(mut self, bytes: usize) -> Self {
        self.rcvbuf_bytes = Some(bytes);
        self
    }

    /// The camera's `PayloadSize`: the exact size of every frame buffer
    /// ([`FrameStream::set_payload_size`]), in place before the first frame.
    /// [`Camera::open_stream`](crate::Camera::open_stream) reads it from the
    /// camera itself; [`GigeRegisterIo::open_stream`](crate::GigeRegisterIo::open_stream),
    /// which has no node map to read it with, sizes each frame from its
    /// leader without it.
    pub fn payload_size(mut self, bytes: usize) -> Self {
        self.payload_size = Some(bytes).filter(|&b| b > 0);
        self
    }
}

impl crate::Camera<crate::GigeRegisterIo> {
    /// Open a GVSP stream from this camera, ready for `acquisition_start`.
    ///
    /// The one recipe every consumer uses (API-17), as
    /// [`GigeRegisterIo::open_stream`](crate::GigeRegisterIo::open_stream)
    /// describes it, with the camera's `PayloadSize` and time sync. A camera
    /// without a readable `PayloadSize` still streams: buffers are then sized
    /// from each leader.
    ///
    /// [`StreamOptions::monitor`] instead watches a multicast stream another
    /// application configured, claiming and writing nothing; it is the only
    /// stream a camera opened with [`Access::Monitor`](crate::Access::Monitor)
    /// can open.
    pub fn open_stream(&mut self, options: StreamOptions) -> Result<FrameStream, GenicamError> {
        if options.monitor {
            return self.watch_stream(options);
        }
        let mut options = options;
        if options.payload_size.is_none() {
            options.payload_size = read_payload_size(self);
        }
        let mut frames = self.transport().open_stream(options)?;
        frames.set_time_sync(self.time_sync().clone());
        Ok(frames)
    }

    /// [`StreamOptions::monitor`]: join the multicast group the camera
    /// already streams to, reading where from its registers. Nothing is
    /// written.
    fn watch_stream(&mut self, options: StreamOptions) -> Result<FrameStream, GenicamError> {
        const CHANNEL: u32 = 0;
        let payload_size = read_payload_size(self);
        let time_sync = self.time_sync().clone();
        let transport = self.transport();
        let mut guard = transport.lock_device()?;
        let device: &mut GigeDevice = &mut guard;
        let (socket, packet_size, group, iface_ip, camera_ip) =
            transport.block_on(async move {
                let IpAddr::V4(camera_ip) = device.remote_addr().ip() else {
                    return Err(GigeError::NotIpv4("camera").into());
                };
                let (group, port) = device
                    .stream_destination(CHANNEL)
                    .await
                    .map_err(gige("read GevSCDA/GevSCPHostPort"))?;
                if !group.is_multicast() || port == 0 {
                    // A monitor can only watch a stream the controlling
                    // application sent to a multicast group.
                    return Err(GenicamError::NotMulticast(std::net::SocketAddrV4::new(
                        group, port,
                    )));
                }
                let packet_size = device
                    .get_stream_packet_size(CHANNEL)
                    .await
                    .map_err(gige("read GevSCPSPacketSize"))?;
                let iface = match options.iface {
                    Some(iface) => iface,
                    None => Iface::from_remote_ipv4(camera_ip).map_err(|source| {
                        GenicamError::NoRoute {
                            camera: camera_ip,
                            source,
                        }
                    })?,
                };
                let iface_ip = iface
                    .ipv4()
                    .ok_or_else(|| GenicamError::InterfaceWithoutIpv4(iface.name().to_string()))?;
                let mc = McOptions {
                    rcvbuf_bytes: options.rcvbuf_bytes.unwrap_or(DEFAULT_RCVBUF_BYTES),
                    ..McOptions::default()
                };
                let socket = viva_gige::bind_multicast(&iface, group, port, &mc)
                    .await
                    .map_err(network("join the camera's multicast group"))?;
                info!(%group, port, packet_size, "watching the camera's multicast stream");
                Ok((socket, packet_size, group, iface_ip, camera_ip))
            })??;
        drop(guard);
        let socket = socket
            .into_std()
            .map_err(network("hand the stream socket to the receive thread"))?;
        let mut frames = FrameStream::watch_multicast(socket, packet_size, group, iface_ip)?;
        frames.set_source_filter(Some(camera_ip));
        frames.set_payload_size(payload_size);
        frames.set_time_sync(time_sync);
        Ok(frames)
    }
}

impl crate::GigeRegisterIo {
    /// Open a GVSP stream from this transport's camera, ready for
    /// acquisition: the recipe [`Camera::open_stream`](crate::Camera::open_stream)
    /// runs, for a caller without a node map — a GenTL producer, which the
    /// GenTL standard (§2.1.2) asks not to run GenApi itself.
    ///
    /// Claims control (again, if it was released) before anything is
    /// written, binds the socket, points the stream channel at it — or at a
    /// multicast group — negotiates the packet size, reads whether the camera
    /// supports resend, and starts the receive engine. Without
    /// [`StreamOptions::payload_size`] each frame is sized from its leader.
    /// [`StreamOptions::monitor`] needs the camera's node map; here it is
    /// refused.
    // The device lock is held for the whole setup, outside the future that
    // borrows the device: nothing else may use the control channel while the
    // stream channel is being configured.
    pub fn open_stream(&self, options: StreamOptions) -> Result<FrameStream, GenicamError> {
        if options.monitor || self.control_state() == crate::ControlState::Monitor {
            return Err(GenicamError::NotInControl(crate::ControlState::Monitor));
        }
        let payload_size = options.payload_size;
        let mut guard = self.lock_device()?;
        let device: &mut GigeDevice = &mut guard;
        let stream = self.block_on(async move {
            device
                .claim_control()
                .await
                .map_err(gige("claim control"))?;
            // A session that had lost or released control holds it again,
            // whatever happens to the setup below: the camera says so.
            self.note_claimed();
            let iface = match options.iface {
                Some(iface) => iface,
                None => {
                    let camera_ip = match device.remote_addr().ip() {
                        IpAddr::V4(ip) => ip,
                        IpAddr::V6(_) => {
                            return Err(GigeError::NotIpv4("camera").into());
                        }
                    };
                    Iface::from_remote_ipv4(camera_ip).map_err(|source| GenicamError::NoRoute {
                        camera: camera_ip,
                        source,
                    })?
                }
            };
            // The stream channel is configured through the bootstrap registers,
            // multicast included, and only now that control is held: a device
            // refuses those writes from an application without privilege.
            let mut builder = StreamBuilder::new(device)
                .iface(iface)
                .resend(options.resend);
            builder.packet_size_mode = options.packet_size;
            builder.packet_delay = options.packet_delay;
            if let Some(bytes) = options.rcvbuf_bytes {
                builder = builder.rcvbuf_bytes(bytes);
            }
            if let Some(group) = options.multicast {
                builder = builder.multicast(Some(group));
            }
            if let Some(port) = options.destination_port {
                builder = builder.destination_port(port);
            }
            builder.build().await
        })??;
        drop(guard);
        Ok(FrameStream::start_with(stream, None, payload_size))
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
    /// Whether the stream asks the camera for lost packets.
    resend: bool,
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

    /// Whether the stream will ask the camera for lost packets.
    pub fn resend_enabled(&self) -> bool {
        self.resend
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
    /// The multicast group the socket joined, left when the stream drops.
    membership: Option<Membership>,
}

/// A multicast group a stream's socket joined.
///
/// Holds a duplicate of the socket, so the group is left explicitly when the
/// stream drops — an IGMP leave the switch sees at once (SR-08) — rather than
/// whenever the last descriptor of the socket happens to close.
struct Membership {
    socket: std::net::UdpSocket,
    group: Ipv4Addr,
    iface: Ipv4Addr,
}

impl Membership {
    fn of(socket: &std::net::UdpSocket, group: Ipv4Addr, iface: Ipv4Addr) -> Option<Self> {
        match socket.try_clone() {
            Ok(socket) => Some(Self {
                socket,
                group,
                iface,
            }),
            Err(err) => {
                debug!(%group, error = %err, "cannot keep the stream socket to leave its group");
                None
            }
        }
    }
}

impl Drop for Membership {
    fn drop(&mut self) {
        match self.socket.leave_multicast_v4(&self.group, &self.iface) {
            Ok(()) => debug!(group = %self.group, "left the multicast group"),
            Err(err) => {
                debug!(group = %self.group, error = %err, "leaving the multicast group failed")
            }
        }
    }
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
            resend,
        } = stream;
        let source_filter = config.source_filter;
        let group = match (&config.dest, config.iface.ipv4()) {
            (StreamDest::Multicast { group, .. }, Some(iface)) => Some((*group, iface)),
            _ => None,
        };
        let mut frames = Self::idle(stats, params, Some(config), time_sync);
        frames.shared.set_payload_size(payload_size);
        frames.shared.set_source_filter(source_filter);
        if resend
            && let Some(SocketAddr::V4(camera)) = params.camera
            && let Ok(channel) = u16::try_from(params.channel)
        {
            frames.shared.set_resend_target(Some((camera, channel)));
        }
        match socket.into_std() {
            Ok(socket) => {
                frames.membership =
                    group.and_then(|(group, iface)| Membership::of(&socket, group, iface));
                frames.start(socket, rcvbuf_bytes);
            }
            Err(err) => frames
                .shared
                .queue
                .fail("hand the stream socket to the receive thread", err),
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
    ///
    /// Such a stream accepts datagrams from any address until
    /// [`set_source_filter`](Self::set_source_filter) names the camera, and
    /// never asks for lost packets unless
    /// [`enable_resend`](Self::enable_resend) is called. A host that only
    /// monitors a stream must not ask: it does not control the camera, and its
    /// requests would add traffic to a stream another application configured.
    pub fn from_socket(
        socket: std::net::UdpSocket,
        packet_size: u32,
    ) -> Result<Self, GenicamError> {
        if assembly::payload_stride(packet_size, viva_gige::GVSP_EXTENDED_HEADER_SIZE) == 0 {
            return Err(GenicamError::PacketTooSmall {
                size: packet_size,
                min: MIN_PACKET_SIZE,
            });
        }
        let local = socket
            .local_addr()
            .map_err(network("read the stream socket's address"))?;
        let host = match local.ip() {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
        };
        let params = StreamParams {
            packet_size,
            packet_delay: None,
            mtu: packet_size,
            host,
            port: local.port(),
            channel: 0,
            camera: None,
        };
        let mut frames = Self::idle(StreamStatsAccumulator::new(), params, None, None);
        frames.start(socket, None);
        Ok(frames)
    }

    /// Receive on `socket`, already joined to `group` on the interface at
    /// `iface`, and leave the group when the stream drops (SR-08).
    pub(crate) fn watch_multicast(
        socket: std::net::UdpSocket,
        packet_size: u32,
        group: Ipv4Addr,
        iface: Ipv4Addr,
    ) -> Result<Self, GenicamError> {
        let membership = Membership::of(&socket, group, iface);
        let mut frames = Self::from_socket(socket, packet_size)?;
        frames.membership = membership;
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
            membership: None,
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
            Err(err) => self.shared.queue.fail("start the GVSP receive thread", err),
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

    /// Accept GVSP only from `camera`; `None` accepts any source.
    ///
    /// A stream from [`StreamBuilder`] or
    /// [`Camera::open_stream`](crate::Camera::open_stream) accepts only the
    /// camera's own address. Datagrams from anywhere else are ignored and
    /// counted in [`StreamStats::packets_ignored_foreign`]; a camera that
    /// sends its stream from another address than the one it answers GVCP on
    /// needs `None`.
    pub fn set_source_filter(&mut self, camera: Option<Ipv4Addr>) {
        self.shared.set_source_filter(camera);
    }

    /// The only address GVSP is accepted from, if any.
    pub fn source_filter(&self) -> Option<Ipv4Addr> {
        self.shared.source_filter()
    }

    /// Ask the camera at `camera` (its GVCP address, normally port 3956) to
    /// send lost packets of stream channel `channel` again.
    ///
    /// [`Camera::open_stream`](crate::Camera::open_stream) does this when the
    /// camera's capability register says it supports packet resend. Call it
    /// yourself only for a stream this application controls, on a camera
    /// that supports resend.
    pub fn enable_resend(&mut self, camera: std::net::SocketAddrV4, channel: u16) {
        self.shared.set_resend_target(Some((camera, channel)));
    }

    /// Stop asking for lost packets.
    pub fn disable_resend(&mut self) {
        self.shared.set_resend_target(None);
    }

    /// Whether the stream asks the camera for lost packets.
    pub fn resend_enabled(&self) -> bool {
        self.shared.resend_target().is_some()
    }

    /// How old a gap in a block's packets must be before it is asked for
    /// (default 2 ms), so a packet that was only overtaken is not.
    pub fn set_resend_delay(&mut self, delay: Duration) {
        self.shared.set_resend_delay(delay);
    }

    /// How long to wait for a resent packet before asking again (default
    /// 20 ms).
    pub fn set_resend_retry_interval(&mut self, interval: Duration) {
        self.shared.set_resend_retry_interval(interval);
    }

    /// How many times one range of packets is asked for, the first request
    /// included (default 3).
    pub fn set_resend_max_requests(&mut self, requests: u32) {
        self.shared.set_resend_max_requests(requests);
    }

    /// The share of a block's packets its requests may ask for, retries
    /// included (default 0.25, rounded up). A block that lost more is not
    /// worth flooding the camera for; it closes incomplete.
    pub fn set_resend_budget(&mut self, share: f64) {
        let share = if share.is_nan() { 0.0 } else { share };
        self.shared.set_resend_budget(share.clamp(0.0, 1.0));
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
            Popped::Failed(during, source) => Err(GenicamError::Network { during, source }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use assembly::LeaderInfo;

    /// SR-08: a stream that joined a multicast group leaves it when it drops,
    /// rather than leaving the membership to the socket's close — which
    /// never comes while anything else holds the socket. A duplicate of the
    /// socket shows it: leaving through it fails once the stream has left.
    #[test]
    fn a_multicast_stream_leaves_its_group_when_dropped() {
        let group = Ipv4Addr::new(239, 192, 1, 31);
        let iface = Ipv4Addr::LOCALHOST;
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("bind");
        if let Err(err) = socket.join_multicast_v4(&group, &iface) {
            eprintln!("skipped: cannot join a group on loopback here: {err}");
            return;
        }
        let duplicate = socket.try_clone().expect("duplicate");
        let stream = FrameStream::watch_multicast(socket, 1500, group, iface).expect("stream");
        drop(stream);
        assert!(
            duplicate.leave_multicast_v4(&group, &iface).is_err(),
            "the group was still joined after the stream dropped"
        );
    }

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
                padding_x: 0,
                timestamp,
                pixel_format,
                width,
                height,
            }),
            payload_type: 0x0001,
            expected_size: Some(payload.len()),
            payload,
            trailer_size_y: Some(trailer_size_y),
        }
    }

    /// SR-19: a delay in time becomes `GevSCPD` ticks at the camera's
    /// frequency, rounded to the nearest tick; no frequency, no conversion.
    #[test]
    fn a_packet_delay_is_converted_to_ticks() {
        let us = Duration::from_micros;
        assert_eq!(delay_in_ticks(us(10), 125_000_000).unwrap(), 1250);
        assert_eq!(delay_in_ticks(us(10), 1_000_000_000).unwrap(), 10_000);
        // 12 ns at 125 MHz is 1.5 ticks; 3 ns is 0.375, which is still not 0.
        assert_eq!(
            delay_in_ticks(Duration::from_nanos(12), 125_000_000).unwrap(),
            2
        );
        assert_eq!(
            delay_in_ticks(Duration::from_nanos(3), 125_000_000).unwrap(),
            1
        );
        assert!(delay_in_ticks(us(10), 0).is_err());
        assert!(delay_in_ticks(Duration::from_secs(10), 1_000_000_000).is_err());
    }

    #[test]
    fn captured_lucid_evt30_metadata_is_classified_as_evs() {
        let payload = Bytes::from_static(&[0x8e, 0x8a, 0xff, 0xe0]);
        let block = classify_completed_block(assembled(
            payload.clone(),
            1,
            64_000,
            0x8110_0e30,
            0x0000_422d_66d4_39c8,
            824,
        ))
        .0;

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
        let block = classify_completed_block(assembled(
            Bytes::from_static(&[0; 8]),
            1,
            64_000,
            0x8140_0e21,
            7,
            8,
        ))
        .0;

        let StreamBlock::Evs(evs) = StreamBlock::from(block) else {
            panic!("EVT 2.1 must not be exposed as an image frame");
        };
        assert_eq!(evs.format, crate::evs::EvsFormat::Evt21);
        assert_eq!(evs.payload.len(), 8);
    }

    #[test]
    fn unknown_pfnc_code_remains_an_image() {
        let pixel_format = PixelFormat::from_code(0x0220_ffff);
        let block = classify_completed_block(assembled(
            Bytes::from_static(&[1, 2, 3, 4]),
            2,
            2,
            pixel_format.code(),
            9,
            2,
        ))
        .0;

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
        let block = classify_completed_block(assembled(
            payload.clone(),
            1,
            64_000,
            PixelFormat::VendorLucidEvsEvt30.code(),
            11,
            3,
        ))
        .0;

        let frame = block.into_legacy_frame();
        assert_eq!(frame.payload, payload);
        assert_eq!((frame.width, frame.height), (1, 64_000));
        assert_eq!(frame.pixel_format, PixelFormat::VendorLucidEvsEvt30);
        assert_eq!(frame.ts_dev, Some(11));
    }

    /// An image with chunks: the frame's payload is the image chunk alone,
    /// and the chunks are kept by ID. Bytes written out by hand: an 8-byte
    /// Mono8 image tagged as chunk 1, then a 4-byte chunk 0x0504_0008.
    #[test]
    fn an_image_with_chunks_is_split_into_image_and_chunks() {
        #[rustfmt::skip]
        let payload = Bytes::from_static(&[
            1, 2, 3, 4, 5, 6, 7, 8,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
            0x88, 0x13, 0x00, 0x00,
            0x05, 0x04, 0x00, 0x08, 0x00, 0x00, 0x00, 0x04,
        ]);
        let mut block = assembled(payload, 4, 2, 0x0108_0001, 5, 2);
        block.payload_type = 0x4001;
        let (block, error) = classify_completed_block(block);
        assert!(error.is_none());
        let StreamBlock::Image(frame) = StreamBlock::from(block) else {
            panic!("an image");
        };
        assert_eq!(frame.payload.as_ref(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        let chunks = frame.chunks.expect("chunks");
        assert_eq!(
            chunks.get(0x0504_0008).map(|c| c.to_vec()),
            Some(vec![0x88, 0x13, 0x00, 0x00])
        );
    }

    /// The raw payload is the whole block, chunks and tags included, for an
    /// image with chunks as for a plain image.
    #[test]
    fn the_raw_payload_is_the_whole_block() {
        #[rustfmt::skip]
        let payload = Bytes::from_static(&[
            1, 2, 3, 4, 5, 6, 7, 8,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
            0x88, 0x13, 0x00, 0x00,
            0x05, 0x04, 0x00, 0x08, 0x00, 0x00, 0x00, 0x04,
        ]);
        for payload_type in [0x4001, 0x0001] {
            let mut block = assembled(payload.clone(), 4, 2, 0x0108_0001, 5, 2);
            block.payload_type = payload_type;
            let (block, _) = classify_completed_block(block);
            let StreamBlock::Image(frame) = StreamBlock::from(block) else {
                panic!("an image");
            };
            assert_eq!(
                frame.raw_payload, payload,
                "payload type {payload_type:#06x}"
            );
            assert!(frame.raw_payload.starts_with(&frame.payload));
        }
    }

    /// A layout that does not parse keeps the whole payload, carries no
    /// chunks, and hands the error back for the receive thread to report.
    #[test]
    fn a_malformed_chunk_layout_is_reported_not_guessed() {
        let mut block = assembled(Bytes::from_static(&[0xFF; 12]), 2, 2, 0x0108_0001, 5, 2);
        block.payload_type = 0x4001;
        let (block, error) = classify_completed_block(block);
        assert!(
            matches!(error, Some(ChunkError::LengthPastStart { .. })),
            "{error:?}"
        );
        let StreamBlock::Image(frame) = StreamBlock::from(block) else {
            panic!("an image");
        };
        assert_eq!(frame.payload.len(), 12);
        assert!(frame.chunks.is_none());
    }

    /// An incomplete block's zeros are not read as chunk tags.
    #[test]
    fn an_incomplete_block_is_not_split() {
        let mut block = assembled(Bytes::from_static(&[0; 16]), 2, 2, 0x0108_0001, 5, 2);
        block.payload_type = 0x4001;
        block.status = FrameStatus::Incomplete { missing_packets: 1 };
        let (block, error) = classify_completed_block(block);
        assert!(error.is_none());
        let StreamBlock::Image(frame) = StreamBlock::from(block) else {
            panic!("an image");
        };
        assert!(frame.chunks.is_none());
        assert_eq!(frame.payload.len(), 16);
    }
}
