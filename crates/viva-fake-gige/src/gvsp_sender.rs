//! GVSP frame generator: sends synthetic image frames as leader + payload + trailer packets.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tracing::{info, trace, warn};

use crate::registers::RegisterMap;

/// GVSP header size in bytes.
const GVSP_HEADER_SIZE: usize = 8;

/// Payload type for image data.
const PAYLOAD_IMAGE: u16 = 0x0001;

/// Run the GVSP frame sender loop.
///
/// Waits for `acq_start_notify`, then streams frames until `stop_flag` is set.
pub async fn run(
    regs: Arc<Mutex<RegisterMap>>,
    acq_start_notify: Arc<Notify>,
    stop_flag: Arc<AtomicBool>,
    fps: u32,
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
        let mut block_id: u16 = 1;
        // Device clock: nanoseconds since acquisition start (1 GHz tick rate).
        let clock_origin = Instant::now();

        loop {
            // Check stop flag.
            if stop_flag.load(Ordering::SeqCst) {
                info!("GVSP streaming stopped");
                break;
            }

            // Read current stream destination and config from registers.
            let (
                dest_ip,
                dest_port,
                packet_size,
                width,
                height,
                pixel_format,
                chunk_active,
                exposure_time,
            ) = {
                let store = regs.lock().await;
                (
                    store.stream_dest_ip(),
                    store.stream_dest_port(),
                    store.stream_packet_size(),
                    store.width(),
                    store.height(),
                    store.pixel_format_code(),
                    store.chunk_mode_active(),
                    store.exposure_time(),
                )
            };

            if dest_port == 0 || dest_ip == Ipv4Addr::UNSPECIFIED {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            let dest = SocketAddr::new(std::net::IpAddr::V4(dest_ip), dest_port);
            let payload_per_packet = (packet_size as usize).saturating_sub(GVSP_HEADER_SIZE);
            if payload_per_packet == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Compute bytes per pixel from pixel format code.
            let bpp: u32 = if pixel_format == 0x02180014 { 3 } else { 1 }; // RGB8 : Mono8
            let image_size = (width * height * bpp) as usize;

            // Generate animated test pattern.
            let image =
                generate_test_pattern(width as usize, height as usize, bpp as usize, block_id);

            // Device timestamp: nanoseconds since acquisition start.
            let timestamp = clock_origin.elapsed().as_nanos() as u64;

            // Send leader packet (packet_id = 0).
            let leader = build_leader(block_id, width, height, pixel_format, timestamp);
            let _ = socket.send_to(&leader, dest).await;

            // Send payload packets.
            let mut packet_id: u16 = 1;
            let mut offset = 0;
            while offset < image_size {
                let end = (offset + payload_per_packet).min(image_size);
                let pkt = build_payload(block_id, packet_id, &image[offset..end]);
                let _ = socket.send_to(&pkt, dest).await;
                packet_id += 1;
                offset = end;
            }

            // Send trailer packet (with optional chunk data).
            let chunk_data = if chunk_active {
                build_chunk_data(timestamp, exposure_time)
            } else {
                Vec::new()
            };
            let trailer = build_trailer(block_id, packet_id, &chunk_data);
            let _ = socket.send_to(&trailer, dest).await;

            trace!(block_id, packets = packet_id + 1, %dest, "frame sent");
            block_id = block_id.wrapping_add(1);
            if block_id == 0 {
                block_id = 1;
            }

            tokio::time::sleep(frame_interval).await;
        }
    }
}

/// Generate a test pattern that animates across frames.
///
/// Mono8: concentric rings radiating from center, shifting with each frame.
/// RGB8: colorful plasma-like pattern with animated hue rotation.
fn generate_test_pattern(width: usize, height: usize, bpp: usize, frame: u16) -> Vec<u8> {
    let size = width * height * bpp;
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

/// Build a GVSP leader packet.
fn build_leader(
    block_id: u16,
    width: u32,
    height: u32,
    pixel_format: u32,
    timestamp: u64,
) -> Vec<u8> {
    // GVSP header (8 bytes) + leader payload (36 bytes)
    let mut buf = BytesMut::with_capacity(44);

    // GVSP header
    buf.put_u16(0); // status = success
    buf.put_u16(block_id); // block_id
    buf.put_u8(0x01); // packet_format: leader
    buf.put_u8(0); // packet_id high byte
    buf.put_u16(0); // packet_id = 0 for leader

    // Leader payload
    buf.put_u16(0); // reserved
    buf.put_u16(PAYLOAD_IMAGE); // payload_type
    buf.put_u64(timestamp); // timestamp
    buf.put_u32(pixel_format); // pixel_format
    buf.put_u32(width); // size_x
    buf.put_u32(height); // size_y
    buf.put_u32(0); // offset_x
    buf.put_u32(0); // offset_y
    buf.put_u16(0); // padding_x
    buf.put_u16(0); // padding_y

    buf.to_vec()
}

/// Build a GVSP payload packet.
fn build_payload(block_id: u16, packet_id: u16, data: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(GVSP_HEADER_SIZE + data.len());

    // GVSP header
    buf.put_u16(0); // status
    buf.put_u16(block_id); // block_id
    buf.put_u8(0x03); // packet_format: payload
                      // packet_id as 3 bytes big-endian (u24): [bits 23:16, bits 15:8, bits 7:0]
    buf.put_u8(0); // bits 23:16 (always 0 for u16 packet_id)
    buf.put_u8((packet_id >> 8) as u8); // bits 15:8
    buf.put_u8(packet_id as u8); // bits 7:0

    buf.put_slice(data);
    buf.to_vec()
}

/// Build a GVSP trailer packet with optional chunk data.
fn build_trailer(block_id: u16, packet_id: u16, chunk_data: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(16 + chunk_data.len());

    // GVSP header
    buf.put_u16(0); // status
    buf.put_u16(block_id); // block_id
    buf.put_u8(0x02); // packet_format: trailer
                      // packet_id as 3 bytes big-endian (u24)
    buf.put_u8(0);
    buf.put_u8((packet_id >> 8) as u8);
    buf.put_u8(packet_id as u8);

    // Trailer payload
    buf.put_u16(0); // reserved/status
    if chunk_data.is_empty() {
        buf.put_u16(PAYLOAD_IMAGE); // payload_type
    } else {
        buf.put_u16(0x4001); // payload_type: image + chunk
    }
    buf.put_u32(0); // size_y

    // Append chunk data blocks
    buf.put_slice(chunk_data);

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
