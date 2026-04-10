//! GVSP frame generator: sends synthetic image frames as leader + payload + trailer packets.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

        loop {
            // Check stop flag.
            if stop_flag.load(Ordering::SeqCst) {
                info!("GVSP streaming stopped");
                break;
            }

            // Read current stream destination from registers.
            let (dest_ip, dest_port, packet_size, width, height, pixel_format) = {
                let store = regs.lock().await;
                (
                    store.stream_dest_ip(),
                    store.stream_dest_port(),
                    store.stream_packet_size(),
                    store.width(),
                    store.height(),
                    store.pixel_format_code(),
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

            // Generate synthetic image (horizontal gradient).
            let image = generate_gradient(width as usize, height as usize, bpp as usize, block_id);

            // Send leader packet (packet_id = 0).
            let leader = build_leader(block_id, width, height, pixel_format);
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

            // Send trailer packet.
            let trailer = build_trailer(block_id, packet_id);
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

/// Generate a gradient test pattern.
fn generate_gradient(width: usize, height: usize, bpp: usize, seed: u16) -> Vec<u8> {
    let size = width * height * bpp;
    let mut data = vec![0u8; size];

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * bpp;
            let val = ((x as u16)
                .wrapping_add(seed)
                .wrapping_mul(255 / width.max(1) as u16)) as u8;
            if bpp == 1 {
                data[offset] = val;
            } else if bpp >= 3 {
                data[offset] = val; // R
                data[offset + 1] = ((y * 255) / height.max(1)) as u8; // G
                data[offset + 2] = seed as u8; // B
            }
        }
    }
    data
}

/// Build a GVSP leader packet.
fn build_leader(block_id: u16, width: u32, height: u32, pixel_format: u32) -> Vec<u8> {
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
    buf.put_u64(0); // timestamp
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
                      // packet_id as 3 bytes (big-endian u24)
    buf.put_u8((packet_id >> 8) as u8);
    buf.put_u16(packet_id);

    buf.put_slice(data);
    buf.to_vec()
}

/// Build a GVSP trailer packet.
fn build_trailer(block_id: u16, packet_id: u16) -> Vec<u8> {
    // GVSP header (8 bytes) + trailer payload (8 bytes)
    let mut buf = BytesMut::with_capacity(16);

    // GVSP header
    buf.put_u16(0); // status
    buf.put_u16(block_id); // block_id
    buf.put_u8(0x02); // packet_format: trailer
    buf.put_u8((packet_id >> 8) as u8);
    buf.put_u16(packet_id); // packet_id = last payload + 1

    // Trailer payload
    buf.put_u16(0); // reserved
    buf.put_u16(PAYLOAD_IMAGE); // payload_type
    buf.put_u32(0); // size_y (can be used for validation)

    buf.to_vec()
}
