//! GVSP frame generator: sends synthetic image frames as leader + payload + trailer packets.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, trace, warn};

use crate::gvsp::{
    BlockSpec, Datagram, PAYLOAD_TYPE_IMAGE, PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK, PacketScript,
    block_datagrams, image_size, interleave,
};
use crate::registers::RegisterMap;

/// IPv4 header size counted by `GevSCPSPacketSize` per the GigE Vision spec.
const IPV4_HEADER_SIZE: usize = 20;
/// UDP header size counted by `GevSCPSPacketSize` per the GigE Vision spec.
const UDP_HEADER_SIZE: usize = 8;

/// GVCP command message key byte.
const GVCP_CMD_KEY: u8 = 0x42;
/// GVCP `EVENT_CMD` opcode.
const EVENT_CMD: u16 = 0x00C0;
/// `GEV_EVENT_START_OF_TRANSFER`, emitted as each frame begins.
const GEV_EVENT_START_OF_TRANSFER: u16 = 0x0005;
/// Size of one 16-bit-block-id event entry.
const EVENT_ENTRY_LEN: u16 = 16;

/// Stream options set on the builder rather than through registers.
#[derive(Debug, Clone, Default)]
pub(crate) struct SenderOptions {
    /// Send 20-byte extended-ID headers with 64-bit block ids.
    pub extended_ids: bool,
    /// Payload type for the leader and trailer; `None` is image, or image
    /// with extended chunk data while chunk mode is active.
    pub payload_type: Option<u16>,
    /// Deterministic damage to apply to the stream.
    pub script: PacketScript,
}

/// Run the GVSP frame sender loop.
///
/// Waits for `acq_start_notify`, then streams frames until `stop_flag` is set.
/// Every block built counts in `blocks_sent`, including one the script then
/// drops whole: that is a block the camera produced and the host never saw.
pub async fn run(
    regs: Arc<Mutex<RegisterMap>>,
    acq_start_notify: Arc<Notify>,
    stop_flag: Arc<AtomicBool>,
    fps: u32,
    options: SenderOptions,
    blocks_sent: Arc<AtomicU64>,
) {
    loop {
        // Wait for acquisition start.
        acq_start_notify.notified().await;
        info!("GVSP streaming started");
        stop_flag.store(false, Ordering::SeqCst);

        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to bind GVSP socket");
                continue;
            }
        };

        let frame_interval = Duration::from_micros(1_000_000 / fps.max(1) as u64);
        let mut block_id: u64 = 1;
        let mut request_id: u16 = 1;
        // Device clock: nanoseconds since acquisition start (1 GHz tick rate).
        let clock_origin = Instant::now();

        loop {
            // Check stop flag.
            if stop_flag.load(Ordering::SeqCst) {
                info!("GVSP streaming stopped");
                break;
            }

            let config = {
                let store = regs.lock().await;
                // A real camera emits the event when EventNotification is On
                // for *this* event and the message channel has a destination.
                // What the controller selected most recently is irrelevant —
                // enabling a second event must not silence this one.
                let event_dest = if store.event_notification_on(GEV_EVENT_START_OF_TRANSFER)
                    && store.message_dest_port() != 0
                    && store.message_dest_ip() != Ipv4Addr::UNSPECIFIED
                {
                    Some(SocketAddr::new(
                        std::net::IpAddr::V4(store.message_dest_ip()),
                        store.message_dest_port(),
                    ))
                } else {
                    None
                };
                FrameConfig {
                    dest_ip: store.stream_dest_ip(),
                    dest_port: store.stream_dest_port(),
                    packet_size: store.stream_packet_size(),
                    packet_delay: packet_delay(
                        store.stream_packet_delay_ticks(),
                        store.tick_frequency(),
                    ),
                    width: store.width(),
                    height: store.height(),
                    pixel_format: store.pixel_format_code(),
                    chunk_active: store.chunk_mode_active(),
                    exposure_time: store.exposure_time(),
                    event_dest,
                    max_on_wire: store.max_on_wire(),
                }
            };

            if config.dest_port == 0 || config.dest_ip == Ipv4Addr::UNSPECIFIED {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let dest = SocketAddr::new(std::net::IpAddr::V4(config.dest_ip), config.dest_port);

            let first = block_id;
            let mut datagrams = match build_block(&config, &options, first, clock_origin) {
                Some(datagrams) => datagrams,
                None => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            block_id = next_block_id(first, options.extended_ids);
            blocks_sent.fetch_add(1, Ordering::Relaxed);
            datagrams = options.script.apply(datagrams);
            if options.script.interleaves(first) {
                let second = block_id;
                if let Some(next) = build_block(&config, &options, second, clock_origin) {
                    datagrams = interleave(datagrams, options.script.apply(next));
                    block_id = next_block_id(second, options.extended_ids);
                    blocks_sent.fetch_add(1, Ordering::Relaxed);
                }
            }

            // GEV_EVENT_START_OF_TRANSFER precedes the leader, as on a real
            // device: the controller learns a block is starting before its
            // first GVSP packet arrives.
            if let Some(event_dest) = config.event_dest {
                let timestamp = clock_origin.elapsed().as_nanos() as u64;
                let event = build_event(
                    request_id,
                    GEV_EVENT_START_OF_TRANSFER,
                    first as u16,
                    timestamp,
                );
                let _ = socket.send_to(&event, event_dest).await;
                request_id = request_id.wrapping_add(1);
                if request_id == 0 {
                    request_id = 1;
                }
            }

            // GevSCPD spaces consecutive packets. Sleeping per packet would be
            // bound by the timer's millisecond granularity, so each packet has
            // a due time on a schedule from the first, and the sender sleeps
            // only once it is a millisecond or more ahead of it.
            let started = Instant::now();
            let mut dropped = 0usize;
            let total = datagrams.len();
            for (index, datagram) in datagrams.iter().enumerate() {
                if !config.packet_delay.is_zero() {
                    let due = started + config.packet_delay * index as u32;
                    let now = Instant::now();
                    if due > now + Duration::from_millis(1) {
                        tokio::time::sleep(due - now).await;
                    }
                }
                if !send_on_wire(&socket, &datagram.bytes, dest, config.max_on_wire).await {
                    dropped += 1;
                }
            }

            if dropped > 0 {
                debug!(
                    block_id = first,
                    dropped,
                    packet_size = config.packet_size,
                    max_on_wire = config.max_on_wire,
                    "packets exceeded the path ceiling and were dropped in flight"
                );
            }
            trace!(block_id = first, packets = total, %dest, "frame sent");

            tokio::time::sleep(frame_interval).await;
        }
    }
}

/// Register state read once per frame.
struct FrameConfig {
    dest_ip: Ipv4Addr,
    dest_port: u16,
    packet_size: u32,
    packet_delay: Duration,
    width: u32,
    height: u32,
    pixel_format: u32,
    chunk_active: bool,
    exposure_time: f64,
    event_dest: Option<SocketAddr>,
    max_on_wire: Option<u32>,
}

/// `GevSCPD` counts timestamp ticks, not nanoseconds: convert with the tick
/// frequency the device reports at bootstrap 0x093C/0x0940.
fn packet_delay(ticks: u32, tick_frequency: u64) -> Duration {
    if ticks == 0 || tick_frequency == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos((ticks as u128 * 1_000_000_000 / tick_frequency as u128) as u64)
}

/// The block after `id`: 16-bit ids wrap from 65535 to 1, skipping 0.
fn next_block_id(id: u64, extended_ids: bool) -> u64 {
    if extended_ids {
        return id.wrapping_add(1).max(1);
    }
    let next = (id as u16).wrapping_add(1);
    if next == 0 { 1 } else { next as u64 }
}

/// Build one block's datagrams from the current register state, or `None`
/// when the packet size leaves no room for data.
fn build_block(
    config: &FrameConfig,
    options: &SenderOptions,
    block_id: u64,
    clock_origin: Instant,
) -> Option<Vec<Datagram>> {
    let payload_type = options.payload_type.unwrap_or(if config.chunk_active {
        PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK
    } else {
        PAYLOAD_TYPE_IMAGE
    });
    let spec = BlockSpec {
        block_id,
        packet_size: config.packet_size,
        extended_ids: options.extended_ids,
        payload_type,
        pixel_format: config.pixel_format,
        width: config.width,
        height: config.height,
        // Device timestamp: nanoseconds since acquisition start.
        timestamp: clock_origin.elapsed().as_nanos() as u64,
        trailer_status: 0,
        size_y: config.height,
    };
    if spec.payload_stride() == 0 {
        return None;
    }
    let size = image_size(config.pixel_format, config.width, config.height);
    let image = generate_test_pattern(
        config.width as usize,
        config.height as usize,
        size,
        block_id as u16,
    );
    let chunk_data = if config.chunk_active {
        build_chunk_data(spec.timestamp, config.exposure_time)
    } else {
        Vec::new()
    };
    Some(block_datagrams(&spec, &image, &chunk_data))
}

/// Send one GVSP datagram, unless a configured path ceiling would have
/// swallowed it. Returns whether it went out.
///
/// `max_on_wire` models a hop that is narrower than either endpoint — a switch,
/// a driver, a NIC — which drops the frame with nobody to report the loss. That
/// is the shape of the link in
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), and it is
/// what no register read can discover.
///
/// It applies to the **stream** as well as to test packets. Gating only the
/// test packets would make the fake agree with the probe's own answer while
/// being physically incapable of contradicting it: a camera left configured one
/// byte above the ceiling would still deliver every frame, so no test could
/// catch it. That is the ADR-0019 failure mode, one level up from the parser.
async fn send_on_wire(
    socket: &UdpSocket,
    packet: &[u8],
    dest: SocketAddr,
    max_on_wire: Option<u32>,
) -> bool {
    // `GevSCPSPacketSize` counts the transmitted IP datagram, so the ceiling is
    // compared against the UDP payload plus both headers.
    let on_wire = packet.len() + IPV4_HEADER_SIZE + UDP_HEADER_SIZE;
    if max_on_wire.is_some_and(|max| on_wire > max as usize) {
        return false;
    }
    socket.send_to(packet, dest).await.is_ok()
}

/// Generate a test pattern of `size` bytes that animates across frames.
///
/// Mono8: concentric rings radiating from center, shifting with each frame.
/// RGB8: colorful plasma-like pattern with animated hue rotation.
/// Any other size (16-bit, packed): a byte ramp, so every packet differs.
fn generate_test_pattern(width: usize, height: usize, size: usize, frame: u16) -> Vec<u8> {
    let pixels = width * height;
    let bpp = if size == pixels {
        1
    } else if size == pixels * 3 {
        3
    } else {
        return (0..size)
            .map(|i| (i as u32).wrapping_mul(7).wrapping_add(frame as u32) as u8)
            .collect();
    };
    let mut data = vec![0u8; size];
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let phase = frame as f32 * 0.15;

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * bpp;
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if bpp == 1 {
                // Concentric rings with radial gradient
                let ring = ((dist * 0.1 - phase) * 2.0).sin();
                let radial = 1.0 - (dist / (cx.max(cy) * 1.2)).min(1.0);
                let val = ((ring * 0.5 + 0.5) * radial * 255.0) as u8;
                data[offset] = val;
            } else if bpp >= 3 {
                // Plasma pattern with hue rotation
                let angle = dy.atan2(dx);
                let v1 = ((dist * 0.05 - phase).sin() + 1.0) * 0.5;
                let v2 = ((angle * 3.0 + phase * 2.0).sin() + 1.0) * 0.5;
                let v3 = (((x as f32 * 0.02 + y as f32 * 0.03 + phase).sin()) + 1.0) * 0.5;

                let hue = (v1 + v2) * 0.5 + phase * 0.1;
                let sat = 0.7 + v3 * 0.3;
                let val = 0.5 + v1 * 0.5;
                let (r, g, b) = hsv_to_rgb(hue % 1.0, sat, val);
                data[offset] = r;
                data[offset + 1] = g;
                data[offset + 2] = b;
            }
        }
    }
    data
}

/// Convert HSV (all 0.0..1.0) to RGB (0..255).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h * 6.0;
    let i = h.floor() as u32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Build a GVCP `EVENT_CMD` carrying one event.
///
/// Written out from the GigE Vision field table, not from the client parser:
/// key byte, flags, opcode, length, request id, then a 16-byte entry of
/// reserved / event id / stream channel / block id / timestamp. Emitting these
/// bytes is what proves `viva-gige::message` reads the right offsets — the
/// previous fake emitted nothing at all, which is why a parser keyed on a
/// non-existent opcode survived.
fn build_event(request_id: u16, event_id: u16, block_id: u16, timestamp: u64) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(8 + EVENT_ENTRY_LEN as usize);
    buf.put_u8(GVCP_CMD_KEY);
    buf.put_u8(0); // no acknowledgement requested, 16-bit block ids
    buf.put_u16(EVENT_CMD);
    buf.put_u16(EVENT_ENTRY_LEN);
    buf.put_u16(request_id);
    buf.put_u16(0); // reserved
    buf.put_u16(event_id);
    buf.put_u16(0); // stream channel index
    buf.put_u16(block_id);
    buf.put_u64(timestamp);
    buf.to_vec()
}

/// Build chunk data blocks: Timestamp (0x0001) + ExposureTime (0x1002).
///
/// Each chunk: [id: u16][reserved: u16][length: u32][data...]
fn build_chunk_data(timestamp: u64, exposure_time: f64) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(32);

    // Chunk: Timestamp (id=0x0001, 8 bytes LE)
    buf.put_u16(0x0001); // chunk id
    buf.put_u16(0); // reserved
    buf.put_u32(8); // data length
    buf.put_u64_le(timestamp); // timestamp value (little-endian per spec)

    // Chunk: ExposureTime (id=0x1002, 8 bytes LE)
    buf.put_u16(0x1002); // chunk id
    buf.put_u16(0); // reserved
    buf.put_u32(8); // data length
    buf.put_f64_le(exposure_time); // exposure time value (little-endian)

    buf.to_vec()
}
