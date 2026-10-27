//! Two-process acquisition benchmark, without camera control or conversion.
//! Run receive first, then send to its printed ephemeral loopback address.
//! receive BIND WIDTH HEIGHT FRAMES TIMEOUT_S OUTPUT.json [async|blocking [PAUSE_AT_FRAME PAUSE_US]]
//! send DEST WIDTH HEIGHT FPS FRAMES PACKET_GAP_US
//! Run each receiver under the OS time/profiler tool; sender CPU is separate.
use std::{
    error::Error,
    io::Write,
    net::UdpSocket,
    str::FromStr,
    time::{Duration, Instant},
};
use viva_fake_gige::{BlockSpec, MONO8, block_datagrams};
use viva_genicam::FrameStream;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn arg<T: FromStr>(args: &[String], at: usize) -> Result<T>
where
    T::Err: Error + 'static,
{
    Ok(args.get(at).ok_or("missing argument")?.parse()?)
}
fn wait_until(deadline: Instant) {
    let left = deadline.saturating_duration_since(Instant::now());
    if left > Duration::from_micros(150) {
        std::thread::sleep(left - Duration::from_micros(100));
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let width: u32 = arg(&args, 3)?;
    let height: u32 = arg(&args, 4)?;
    let size = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n > 0 && n <= 64 * 1024 * 1024)
        .ok_or("invalid geometry")?;
    match args.get(1).map(String::as_str) {
        Some("send") => {
            let dest: std::net::SocketAddr = arg(&args, 2)?;
            if !dest.ip().is_loopback() {
                return Err("synthetic sender requires loopback".into());
            }
            let fps: f64 = arg(&args, 5)?;
            let count: u64 = arg(&args, 6)?;
            let gap_us: f64 = arg(&args, 7)?;
            if !fps.is_finite() || fps <= 0.0 || !gap_us.is_finite() || gap_us < 0.0 || count == 0 {
                return Err("invalid rate/count".into());
            }
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let mut packets =
                block_datagrams(&BlockSpec::image(1, width, height, MONO8, 1500), &data);
            let socket = UdpSocket::bind("127.0.0.1:0")?;
            let start = Instant::now();
            let gap = Duration::from_secs_f64(gap_us / 1e6);
            for frame in 0..count {
                wait_until(start + Duration::from_secs_f64(frame as f64 / fps));
                let id = (frame % 65535 + 1) as u16;
                let mut packet_due = Instant::now();
                for packet in &mut packets {
                    packet.bytes[2..4].copy_from_slice(&id.to_be_bytes());
                    if !gap.is_zero() {
                        wait_until(packet_due);
                        packet_due += gap;
                    }
                    socket.send_to(&packet.bytes, dest)?;
                }
            }
            println!(
                "{}",
                serde_json::json!({"sent":count,"elapsed_s":start.elapsed().as_secs_f64(),"width":width,"height":height,"requested_fps":fps,"packet_gap_us":gap_us,"packets_per_frame":packets.len()})
            );
        }
        Some("receive") => {
            let bind: std::net::SocketAddr = arg(&args, 2)?;
            if !bind.ip().is_loopback() {
                return Err("synthetic receiver requires loopback".into());
            }
            let expected: u64 = arg(&args, 5)?;
            let timeout_s: f64 = arg(&args, 6)?;
            let output = args.get(7).ok_or("missing output")?;
            if expected == 0 || !timeout_s.is_finite() || timeout_s <= 0.0 {
                return Err("invalid count/timeout".into());
            }
            let blocking = match args.get(8).map(String::as_str) {
                None | Some("async") => false,
                Some("blocking") => true,
                _ => return Err("expected async or blocking delivery".into()),
            };
            let pause_at: u64 = args.get(9).map_or(Ok(0), |s| s.parse())?;
            let pause_us: u64 = args.get(10).map_or(Ok(0), |s| s.parse())?;
            let socket = UdpSocket::bind(bind)?;
            let address = socket.local_addr()?;
            let mut stream = FrameStream::from_socket(socket, 1500)?;
            stream.set_payload_size(Some(size));
            println!("READY {address}");
            std::io::stdout().flush()?;
            let timeout = Duration::from_secs_f64(timeout_s);
            let stop = Instant::now() + timeout;
            let deadline = tokio::time::sleep(timeout);
            tokio::pin!(deadline);
            let mut received = 0u64;
            let mut bad = 0u64;
            let mut first = None;
            let mut last = None;
            let mut max_delivery_gap = Duration::ZERO;
            let mut last_block_id = None;
            loop {
                let frame = if blocking {
                    let remaining = stop.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match stream.next_frame_blocking(remaining) {
                        Err(viva_genicam::GenicamError::Timeout(_)) => break,
                        other => other?,
                    }
                } else {
                    tokio::select! {
                        _ = &mut deadline => break,
                        frame = stream.next_frame() => frame?,
                    }
                };
                let Some(frame) = frame else {
                    break;
                };
                let now = Instant::now();
                first.get_or_insert(now);
                if let Some(previous) = last.replace(now) {
                    max_delivery_gap = max_delivery_gap.max(now.duration_since(previous));
                }
                received += 1;
                last_block_id = Some(frame.block_id);
                let bytes = std::hint::black_box(frame.payload.as_ref());
                if frame.width != width
                    || frame.height != height
                    || bytes.len() != size
                    || !frame.is_complete()
                    || [0, size / 2, size - 1]
                        .into_iter()
                        .any(|at| bytes.get(at).copied() != Some((at % 251) as u8))
                {
                    bad += 1;
                }
                if received >= expected {
                    break;
                }
                // A loss case must not include a post-sender idle timeout in
                // its CPU window. Check resolved blocks only at a possible
                // final ID, including when the 16-bit ID has wrapped.
                if frame.block_id == (expected - 1) % 65535 + 1 {
                    let stats = stream.stats();
                    let resolved = stats.frames_complete
                        + stats.frames_incomplete
                        + stats.frames_missed
                        + stats.frames_backpressure
                        + stats.frames_unsupported;
                    if resolved >= expected {
                        break;
                    }
                }
                if received == pause_at && pause_us > 0 {
                    std::thread::sleep(Duration::from_micros(pause_us));
                }
            }
            let elapsed = first.zip(last).map_or(0.0, |(first, last)| {
                last.duration_since(first).as_secs_f64()
            });
            let stats = stream.stats();
            let passed = received == expected
                && bad == 0
                && stats.frames_incomplete == 0
                && stats.frames_missed == 0
                && stats.frames_backpressure == 0;
            let result = serde_json::json!({"delivery_mode":if blocking {"blocking"} else {"async"},"pause_at_frame":pause_at,"pause_us":pause_us,"max_delivery_gap_ms":max_delivery_gap.as_secs_f64()*1000.0,"last_block_id":last_block_id,"passed":passed,"pid":std::process::id(),"width":width,"height":height,"expected":expected,"received":received,"bad_frames":bad,"elapsed_s":elapsed,"fps":received.saturating_sub(1) as f64 / elapsed,"complete":stats.frames_complete,"incomplete":stats.frames_incomplete,"missed":stats.frames_missed,"backpressure":stats.frames_backpressure,"packets_received":stats.packets_received,"packets_missing":stats.packets_missing,"resend_requested":stats.resend_requested,"resend_recovered":stats.resend_recovered});
            std::fs::write(output, serde_json::to_vec_pretty(&result)?)?;
            println!("{result}");
            if !passed {
                return Err("stream loss or integrity check failure; inspect JSON".into());
            }
        }
        _ => return Err("expected receive or send mode".into()),
    }
    Ok(())
}
