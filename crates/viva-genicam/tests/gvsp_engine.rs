//! GVSP receive engine, driven by scripted datagrams on ephemeral loopback
//! ports. No GVCP, no fake camera process, and nothing binds UDP 3956, so this
//! suite can run beside the fake-camera suites.
//!
//! The datagrams come from `viva_fake_gige::gvsp`, which writes them from the
//! GigE Vision field tables with the fake's own constants (ADR-0019), so the
//! receiver is checked against an independent reading of the wire.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use viva_fake_gige::{BlockSpec, Datagram, block_datagrams, chunk_payload, image_size, interleave};
use viva_fake_gige::{MONO8, MONO12_PACKED, PacketScript, ScriptStep};
use viva_genicam::StreamStats;
use viva_genicam::{FrameStatus, FrameStream, GenicamError};

/// Sends datagrams to a `FrameStream` listening on its own ephemeral port.
struct Harness {
    stream: FrameStream,
    sender: UdpSocket,
    dest: SocketAddr,
}

impl Harness {
    fn new(packet_size: u32) -> Self {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        let dest = rx.local_addr().expect("receiver address");
        let stream = FrameStream::from_socket(rx, packet_size).expect("frame stream");
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
        Self {
            stream,
            sender,
            dest,
        }
    }

    fn send(&self, datagrams: &[Datagram]) {
        for d in datagrams {
            self.sender.send_to(&d.bytes, self.dest).expect("send");
        }
    }

    /// Wait until the counters satisfy `done`: the receive thread runs on its
    /// own, so a test that sent everything waits for it to catch up.
    fn settle(&self, done: impl Fn(&StreamStats) -> bool) -> StreamStats {
        let deadline = Instant::now() + WAIT;
        loop {
            let stats = self.stream.stats();
            if done(&stats) || Instant::now() > deadline {
                return stats;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Send from a thread, `gap` apart, so the receiver runs concurrently;
    /// the thread returns how the sends were actually spaced.
    fn send_paced(
        &self,
        datagrams: Vec<Datagram>,
        gap: Duration,
    ) -> std::thread::JoinHandle<Paced> {
        let sender = self.sender.try_clone().expect("clone sender");
        let dest = self.dest;
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut last: Option<Instant> = None;
            let mut longest_gap = Duration::ZERO;
            let count = datagrams.len();
            for (i, d) in datagrams.into_iter().enumerate() {
                let now = Instant::now();
                if let Some(last) = last.replace(now) {
                    longest_gap = longest_gap.max(now - last);
                }
                sender.send_to(&d.bytes, dest).expect("send");
                if i + 1 < count {
                    std::thread::sleep(gap);
                }
            }
            Paced {
                longest_gap,
                total: started.elapsed(),
            }
        })
    }
}

/// How [`Harness::send_paced`] spaced its sends.
struct Paced {
    /// The longest time between two sends.
    longest_gap: Duration,
    /// From the first send to the last.
    total: Duration,
}

/// Deterministic, non-repeating image bytes.
fn image(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| {
            (i as u32)
                .wrapping_mul(31)
                .wrapping_add(seed as u32 * 7 + (i as u32 >> 8)) as u8
        })
        .collect()
}

const WAIT: Duration = Duration::from_secs(3);

/// Extended-ID headers are 20 bytes, so the stride is 12 bytes shorter than
/// with the 8-byte header. Placing packets at the standard stride (SR-14 and
/// the extended-ID gap in the inventory) leaves a 12-byte hole after every
/// packet and runs the payload long.
#[tokio::test]
async fn extended_ids_place_each_packet_at_its_own_stride() {
    let mut h = Harness::new(576);
    let mut spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    spec.extended_ids = true;
    let img = image(64 * 32, 1);
    h.send(&block_datagrams(&spec, &img));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("an extended-ID frame must complete")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload.len(), img.len());
    assert!(
        frame.payload.as_ref() == img.as_slice(),
        "payload differs from what was sent"
    );
    assert_eq!(frame.block_id, 1);
    assert_eq!(frame.status, FrameStatus::Complete);
}

/// The block timeout counts from the last packet. A frame whose packets keep
/// arriving is healthy however long it takes; counting from the leader (the
/// 0.8 rule) drops one that takes longer than the timeout, and every such
/// frame is lost.
///
/// A loaded runner can oversleep even a 50 ms gap past a 500 ms timeout. Such
/// a send schedule tests inactivity, not a slow healthy transfer. Qualify the
/// measured schedule first, with at most three increasingly tolerant fixtures;
/// the first valid fixture must complete, with no retry of an engine failure.
/// Every fixture spans over three block timeouts and more than a second, so
/// the count-from-leader rule still fails it.
#[tokio::test]
async fn a_slow_transfer_with_small_gaps_completes() {
    // 40 payload packets at a 540-byte stride.
    let spec = BlockSpec::image(1, 216, 100, MONO8, 576);
    let img = image(216 * 100, 2);
    let datagrams = block_datagrams(&spec, &img);
    assert_eq!(datagrams.len(), 42);
    let mut rejected = Vec::new();
    for block_timeout in [
        Duration::from_millis(500),
        Duration::from_secs(1),
        Duration::from_secs(2),
    ] {
        let mut h = Harness::new(576);
        h.stream.set_block_timeout(block_timeout);
        // The receiver runs on its own OS thread. Finish measuring the sends
        // before judging its queued result, so a scheduler-invalid fixture
        // cannot turn into an engine timeout assertion first.
        let paced = h
            .send_paced(datagrams.clone(), block_timeout / 10)
            .join()
            .expect("sender");
        if paced.longest_gap >= block_timeout {
            eprintln!(
                "scheduler-invalid fixture: longest gap {:?}, timeout {:?}, total {:?}",
                paced.longest_gap, block_timeout, paced.total
            );
            rejected.push((block_timeout, paced.longest_gap, paced.total));
            continue;
        }
        assert!(
            paced.total > block_timeout * 3 && paced.total > Duration::from_secs(1),
            "the transfer took {:?} against {:?}, too short to tell the rules apart",
            paced.total,
            block_timeout
        );
        let frame = tokio::time::timeout(Duration::from_secs(20), h.stream.next_frame())
            .await
            .expect("a frame whose packets keep arriving must complete")
            .expect("stream error")
            .expect("stream ended");
        assert!(frame.payload.as_ref() == img.as_slice());
        assert_eq!(frame.status, FrameStatus::Complete);
        return;
    }
    panic!("all three sender schedules exceeded their inactivity timeout: {rejected:?}");
}

/// A trailer can overtake the last data packet. It says how many packets the
/// block has; it is not a verdict that the block is finished.
#[tokio::test]
async fn a_trailer_before_the_last_payload_completes_the_frame() {
    let mut h = Harness::new(576);
    let spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    let img = image(64 * 32, 3);
    let script = PacketScript::new().step(ScriptStep::TrailerBeforeLastPayload { block: 1 });
    h.send(&script.apply(block_datagrams(&spec, &img)));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame must complete when its last packet arrives after the trailer")
        .expect("stream error")
        .expect("stream ended");
    assert!(frame.payload.as_ref() == img.as_slice());
}

/// A leader repeated for a block already in flight changes nothing; resetting
/// the frame on it throws away every packet received so far.
#[tokio::test]
async fn a_repeated_leader_does_not_reset_the_frame() {
    let mut h = Harness::new(576);
    let spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    let img = image(64 * 32, 4);
    let script = PacketScript::new().step(ScriptStep::RepeatLeader { block: 1, after: 2 });
    h.send(&script.apply(block_datagrams(&spec, &img)));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("a repeated leader must not cost the frame")
        .expect("stream error")
        .expect("stream ended");
    assert!(frame.payload.as_ref() == img.as_slice());
    assert_eq!(h.stream.stats().packets_duplicate, 1);
}

/// `Mono12Packed` is 1.5 bytes a pixel, which no whole bytes-per-pixel
/// expresses. The 0.8 Windows reader sized it as `w × h × 1` and truncated a
/// third of every frame; the shared engine must deliver what was sent.
#[tokio::test]
async fn a_packed_frame_is_not_truncated() {
    let mut h = Harness::new(576);
    let spec = BlockSpec::image(1, 64, 32, MONO12_PACKED, 576);
    let size = image_size(MONO12_PACKED, 64, 32);
    assert_eq!(size, 64 * 32 * 3 / 2);
    let img = image(size, 5);
    h.send(&block_datagrams(&spec, &img));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("a packed frame must complete")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload.len(), size, "packed payload truncated");
    assert!(frame.payload.as_ref() == img.as_slice());
}

/// Two blocks whose packets alternate on the wire both complete.
#[tokio::test]
async fn interleaved_blocks_both_complete() {
    let mut h = Harness::new(576);
    let a = image(64 * 32, 6);
    let b = image(64 * 32, 7);
    let first = block_datagrams(&BlockSpec::image(1, 64, 32, MONO8, 576), &a);
    let second = block_datagrams(&BlockSpec::image(2, 64, 32, MONO8, 576), &b);
    h.send(&interleave(first, second));

    for (id, expected) in [(1, &a), (2, &b)] {
        let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
            .await
            .expect("both interleaved frames must complete")
            .expect("stream error")
            .expect("stream ended");
        assert_eq!(frame.block_id, id, "frames leave in the order they started");
        assert!(frame.payload.as_ref() == expected.as_slice());
    }
}

fn small_block(block_id: u64) -> Vec<Datagram> {
    block_datagrams(
        &BlockSpec::image(block_id, 32, 32, MONO8, 576),
        &image(32 * 32, block_id as u8),
    )
}

/// The frame rate is the camera's, not the application's: frames 40 ms
/// apart read in one burst still report about 25 fps. Measured at delivery,
/// a burst read as thousands of frames a second — `viva-camctl bench`
/// reported 10 742 fps for a Manta G-125B at 30.9 fps when lost packets held
/// frames back behind an older one.
#[tokio::test]
async fn the_frame_rate_is_measured_where_frames_complete_not_where_they_are_read() {
    let mut h = Harness::new(576);
    // A loaded runner oversleeps, so the bounds come from the gaps the
    // frames were actually sent with.
    let mut sent = Vec::new();
    for block in 1..=4 {
        sent.push(std::time::Instant::now());
        h.send(&small_block(block));
        std::thread::sleep(Duration::from_millis(40));
    }
    let gaps: Vec<f64> = sent
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64())
        .collect();
    let slowest = 1.0 / gaps.iter().cloned().fold(f64::MIN, f64::max);
    let fastest = 1.0 / gaps.iter().cloned().fold(f64::MAX, f64::min);
    h.settle(|s| s.packets_received >= 4 * 4);
    for _ in 0..4 {
        tokio::time::timeout(WAIT, h.stream.next_frame())
            .await
            .expect("a frame")
            .expect("frame")
            .expect("stream open");
    }
    let fps = h.stream.stats().avg_fps;
    assert!(
        (slowest / 2.0..fastest * 2.0).contains(&fps),
        "{fps} fps for frames sent at {slowest:.1}..{fastest:.1} fps"
    );
}

/// 16-bit block ids run 1..=65535 and skip 0. 65535 → 1 is the next block,
/// not a gap; 1 → 3 misses one block and 3 → 7 misses three.
#[test]
fn block_id_gaps_are_counted_wrap_aware() {
    let mut h = Harness::new(576);
    for id in [65533u64, 65534, 65535, 1, 3, 7] {
        h.send(&small_block(id));
        h.stream
            .next_block_blocking(WAIT)
            .expect("every sent block completes")
            .expect("stream running");
    }
    let stats = h.stream.stats();
    assert_eq!(
        stats.frames_missed, 4,
        "four block ids were skipped: {stats:?}"
    );
    assert_eq!(stats.frames_complete, 6);
}

/// Extended ids are 64 bits and do not wrap.
#[test]
fn extended_block_id_gaps_are_counted() {
    let mut h = Harness::new(576);
    for id in [1u64 << 33, (1 << 33) + 1, (1 << 33) + 5] {
        let mut spec = BlockSpec::image(id, 32, 32, MONO8, 576);
        spec.extended_ids = true;
        h.send(&block_datagrams(&spec, &image(1024, 1)));
        h.stream
            .next_block_blocking(WAIT)
            .expect("every sent block completes")
            .expect("stream running");
    }
    assert_eq!(h.stream.stats().frames_missed, 3);
}

/// Every block the camera sent is accounted for exactly once:
/// sent = complete + incomplete + missed + backpressure.
#[test]
fn every_sent_block_is_accounted_for_under_scripted_loss() {
    let mut h = Harness::new(576);
    let script = PacketScript::new()
        .step(ScriptStep::Drop {
            block: 3,
            packet: 1,
        })
        .step(ScriptStep::DropBlock { block: 5 })
        .step(ScriptStep::Duplicate {
            block: 7,
            packet: 1,
        })
        .step(ScriptStep::Delay {
            block: 9,
            packet: 1,
            by: 1,
        })
        .step(ScriptStep::Drop {
            block: 10,
            packet: 3,
        }); // its trailer
    const SENT: u64 = 12;
    let mut complete = 0;
    for id in 1..=SENT {
        h.send(&script.apply(small_block(id)));
        // Take what is ready as it comes, as an application keeping up would,
        // so the four-frame queue never has to drop one.
        while let Ok(Some(_)) = h.stream.next_block_blocking(Duration::from_millis(20)) {
            complete += 1;
        }
    }
    while complete < 9 {
        h.stream
            .next_block_blocking(WAIT)
            .expect("nine blocks arrive whole")
            .expect("stream running");
        complete += 1;
    }
    // Block 10 lost its trailer, so blocks 11 and 12 waited for it to time
    // out; by the ninth frame every block's fate is decided.
    let stats = h.settle(|s| s.frames_incomplete == 2);
    assert_eq!(
        stats.frames_complete
            + stats.frames_incomplete
            + stats.frames_missed
            + stats.frames_backpressure
            + stats.frames_unsupported,
        SENT,
        "{stats:?}"
    );
    assert_eq!(
        (
            stats.frames_complete,
            stats.frames_incomplete,
            stats.frames_missed
        ),
        (9, 2, 1),
        "{stats:?}"
    );
    assert_eq!(stats.packets_duplicate, 1, "{stats:?}");
    // Block 3 lost data packet 1; block 10 lost its trailer.
    assert_eq!(stats.packets_missing, 2, "{stats:?}");
}

/// Incomplete frames are counted always and delivered only on request, with
/// a status that says what was lost.
#[tokio::test]
async fn incomplete_frames_are_delivered_only_when_asked() {
    let mut h = Harness::new(576);
    let script = PacketScript::new().step(ScriptStep::Drop {
        block: 1,
        packet: 1,
    });
    h.send(&script.apply(small_block(1)));
    h.send(&small_block(2));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the complete block arrives")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(
        frame.block_id, 2,
        "block 1 was incomplete and not delivered"
    );
    assert_eq!(h.stream.stats().frames_incomplete, 1);

    let mut h = Harness::new(576);
    h.stream.set_deliver_incomplete(true);
    h.send(&script.apply(small_block(1)));
    h.send(&small_block(2));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the incomplete block is delivered")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.block_id, 1);
    assert_eq!(frame.status, FrameStatus::Incomplete { missing_packets: 1 });
    assert!(!frame.is_complete());
    let expected = image(32 * 32, 1);
    assert_eq!(
        &frame.payload[540..],
        &expected[540..],
        "what arrived is in place"
    );
    assert!(
        frame.payload[..540].iter().all(|b| *b == 0),
        "what was lost is zero"
    );
}

/// A consumer that falls behind loses the oldest frames, counted as
/// backpressure, and receives the newest four.
#[test]
fn a_slow_consumer_loses_the_oldest_frames_and_counts_them() {
    let mut h = Harness::new(576);
    let datagrams: Vec<Datagram> = (1..=22).flat_map(small_block).collect();
    h.send(&datagrams);
    let stats = h.settle(|s| s.frames_backpressure == 6);
    assert_eq!(stats.frames_backpressure, 6, "{stats:?}");
    let ids: Vec<u64> = (0..16)
        .map(|_| {
            h.stream
                .next_block_blocking(WAIT)
                .expect("a queued block")
                .expect("stream running")
                .block_id()
        })
        .collect();
    assert_eq!(ids, (7..=22).collect::<Vec<_>>());
    let stats = h.stream.stats();
    assert_eq!(stats.frames_complete + stats.frames_backpressure, 22);
    assert!(matches!(
        h.stream.next_block_blocking(Duration::from_millis(20)),
        Err(GenicamError::Timeout(_))
    ));
}

/// The blocking front end needs no async runtime.
#[test]
fn the_blocking_front_end_runs_without_a_runtime() {
    let mut h = Harness::new(576);
    h.send(&small_block(1));
    let block = h
        .stream
        .next_block_blocking(WAIT)
        .expect("a block")
        .expect("stream running");
    assert_eq!(block.block_id(), 1);
    assert_eq!(block.status(), FrameStatus::Complete);
}

/// Dropping the stream stops and joins its receive thread, which closes the
/// socket: the port can be bound again at once.
#[test]
fn dropping_the_stream_joins_the_receiver_and_frees_the_port() {
    let h = Harness::new(576);
    let port = h.dest;
    let started = Instant::now();
    drop(h);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "join took {:?}",
        started.elapsed()
    );
    UdpSocket::bind(port).expect("the receiver released its socket");
}

/// TC-05: `0x4001`, an image with extended chunk data, is assembled as an
/// image and keeps its chunk bit.
#[tokio::test]
async fn an_extended_chunk_image_keeps_its_payload_type() {
    let mut h = Harness::new(576);
    let mut spec = BlockSpec::image(1, 32, 32, MONO8, 576);
    spec.payload_type = 0x4001;
    h.send(&block_datagrams(&spec, &image(1024, 1)));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("an 0x4001 block is an image")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload_type, 0x4001);
}

/// TC-05: other payload types are counted, not dropped at `trace`.
#[test]
fn an_unsupported_payload_type_is_counted() {
    let h = Harness::new(576);
    let mut spec = BlockSpec::image(1, 32, 32, MONO8, 576);
    spec.payload_type = 0x0003; // file
    h.send(&block_datagrams(&spec, &image(1024, 1)));
    let stats = h.settle(|s| s.frames_unsupported == 1);
    assert_eq!(stats.frames_unsupported, 1, "{stats:?}");
    // The leader and two data packets; the trailer closes the block.
    assert_eq!(stats.packets_ignored_unsupported, 3, "{stats:?}");
}

/// The timeout is the caller's: a block silent for longer than it closes.
#[tokio::test]
async fn the_block_timeout_is_adjustable() {
    let mut h = Harness::new(576);
    h.stream.set_block_timeout(Duration::from_millis(20));
    h.stream.set_deliver_incomplete(true);
    let datagrams = small_block(1);
    let sender = h.send_paced(datagrams, Duration::from_millis(200));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the block closes on the short timeout")
        .expect("stream error")
        .expect("stream ended");
    sender.join().unwrap();
    assert!(!frame.is_complete(), "{:?}", frame.status);
}

/// `PayloadSize` sizes each frame buffer. One read before the image grew —
/// Width raised after the stream opened — must not fail every frame.
#[tokio::test]
async fn a_stale_payload_size_still_delivers_whole_frames() {
    let mut h = Harness::new(576);
    h.stream.set_payload_size(Some(100));
    let img = image(32 * 32, 1);
    h.send(&small_block(1));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame completes")
        .expect("stream error")
        .expect("stream ended");
    assert!(frame.payload.as_ref() == img.as_slice());
    assert_eq!(h.stream.stats().packets_ignored_out_of_range, 0);
}

/// The stream can be shared with, and moved to, another thread — what the
/// Python bindings need to drop `unsendable`.
#[test]
fn a_frame_stream_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FrameStream>();
}

/// A data stride smaller than ours: the camera believes 500, we believe 576.
/// Every full packet is short; written at our stride the frame has holes and
/// shifted data, so it must not be delivered as complete (Codex review of
/// #194).
#[tokio::test]
async fn a_frame_of_short_packets_is_not_complete() {
    let mut h = Harness::new(576);
    h.stream.set_deliver_incomplete(true);
    let mut spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    spec.packet_size = 500;
    h.send(&block_datagrams(&spec, &image(64 * 32, 1)));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the block closes")
        .expect("stream error")
        .expect("stream ended");
    assert!(!frame.is_complete(), "{:?}", frame.status);
    let stats = h.settle(|s| s.frames_incomplete == 1);
    assert_eq!((stats.frames_complete, stats.frames_incomplete), (0, 1));
    // Packets 1..=4 are short and not the last; packet 5 is the last.
    assert_eq!(stats.packets_size_mismatch, 4, "{stats:?}");
}

// ---------------------------------------------------------------------------
// Packet resend and the camera as the only source (milestone S)
// ---------------------------------------------------------------------------

/// The camera's control port, as far as resend is concerned: it reads
/// `PACKETRESEND_CMD`s with the test's own offsets, written out below from
/// the field table Wireshark's `packet-gvcp.c` dissects, and answers by
/// sending datagrams from the harness's GVSP sender.
struct ControlPort {
    socket: UdpSocket,
}

/// A `PACKETRESEND_CMD`, as the test reads it.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    channel: u16,
    block_id: u64,
    first: u32,
    last: u32,
    from: SocketAddr,
}

impl ControlPort {
    fn new() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind control port");
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("read timeout");
        Self { socket }
    }

    fn addr(&self) -> SocketAddrV4 {
        match self.socket.local_addr().expect("control address") {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => unreachable!("bound to IPv4"),
        }
    }

    /// The next request within `within`, its raw bytes checked against the
    /// header every `PACKETRESEND_CMD` carries.
    fn next(&self, within: Duration) -> Option<(Request, Vec<u8>)> {
        let deadline = Instant::now() + within;
        let mut buf = [0u8; 2048];
        while Instant::now() < deadline {
            let Ok((len, from)) = self.socket.recv_from(&mut buf) else {
                continue;
            };
            let bytes = buf[..len].to_vec();
            assert_eq!(bytes[0], 0x42, "GVCP command key");
            assert_eq!(
                bytes[1] & 0x01,
                0,
                "PACKETRESEND asks for no acknowledgement"
            );
            assert_eq!(&bytes[2..4], &[0x00, 0x40], "PACKETRESEND_CMD");
            assert_ne!(&bytes[6..8], &[0, 0], "request id 0 is not used");
            let extended = bytes[1] & 0x10 != 0;
            let length = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
            assert_eq!(length, if extended { 20 } else { 12 });
            assert_eq!(len, 8 + length);
            let p = &bytes[8..];
            let word = |at: usize| u32::from_be_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]]);
            let request = if extended {
                Request {
                    channel: u16::from_be_bytes([p[0], p[1]]),
                    block_id: u64::from_be_bytes(p[12..20].try_into().unwrap()),
                    first: word(4),
                    last: word(8),
                    from,
                }
            } else {
                Request {
                    channel: u16::from_be_bytes([p[0], p[1]]),
                    block_id: u64::from(u16::from_be_bytes([p[2], p[3]])),
                    first: word(4) & 0x00FF_FFFF,
                    last: word(8) & 0x00FF_FFFF,
                    from,
                }
            };
            return Some((request, bytes));
        }
        None
    }
}

/// A harness whose stream accepts only 127.0.0.1 and asks `control` for
/// lost packets on stream channel `channel`.
fn resend_harness(packet_size: u32, channel: u16) -> (Harness, ControlPort) {
    let mut h = Harness::new(packet_size);
    let control = ControlPort::new();
    h.stream.set_source_filter(Some(Ipv4Addr::LOCALHOST));
    h.stream.enable_resend(control.addr(), channel);
    (h, control)
}

/// A lost data packet is asked for once the gap is older than the resend
/// delay, in the 12-byte standard layout, from the stream socket — and the
/// frame completes when it is sent again.
#[tokio::test]
async fn a_lost_packet_is_asked_for_in_the_standard_layout_and_recovered() {
    let (mut h, control) = resend_harness(576, 3);
    let img = image(64 * 32, 3);
    let datagrams = block_datagrams(&BlockSpec::image(0x0102, 64, 32, MONO8, 576), &img);
    let lost = datagrams[2].clone(); // data packet 2
    let script = PacketScript::new().step(ScriptStep::Drop {
        block: 0x0102,
        packet: 2,
    });
    h.send(&script.apply(datagrams));

    let (request, bytes) = control.next(WAIT).expect("a resend request");
    assert_eq!(
        request,
        Request {
            channel: 3,
            block_id: 0x0102,
            first: 2,
            last: 2,
            from: h.dest,
        },
        "asked from the stream socket"
    );
    #[rustfmt::skip]
    assert_eq!(&bytes[8..], &[
        0x00, 0x03,             // stream channel 3
        0x01, 0x02,             // block id
        0x00, 0x00, 0x00, 0x02, // first packet id
        0x00, 0x00, 0x00, 0x02, // last packet id
    ]);
    h.send(&[lost]);

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame completes once the packet is resent")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.status, FrameStatus::Complete);
    assert!(frame.payload.as_ref() == img.as_slice());
    let stats = h.stream.stats();
    assert_eq!(
        (stats.resend_requested, stats.resend_recovered),
        (1, 1),
        "{stats:?}"
    );
}

/// With extended ids the request is 20 bytes, flag `0x10`: channel, reserved,
/// 32-bit first and last packet ids, then the 64-bit block id.
#[tokio::test]
async fn an_extended_id_request_carries_the_64_bit_block_id() {
    let (mut h, control) = resend_harness(576, 0);
    let block = 0x0001_0000_0000_0007u64;
    let mut spec = BlockSpec::image(block, 64, 32, MONO8, 576);
    spec.extended_ids = true;
    let img = image(64 * 32, 4);
    let datagrams = block_datagrams(&spec, &img);
    let lost: Vec<Datagram> = datagrams[2..4].to_vec(); // data packets 2 and 3
    let script = PacketScript::new()
        .step(ScriptStep::Drop { block, packet: 2 })
        .step(ScriptStep::Drop { block, packet: 3 });
    h.send(&script.apply(datagrams));

    let (request, bytes) = control.next(WAIT).expect("a resend request");
    assert_eq!(
        (request.block_id, request.first, request.last),
        (block, 2, 3)
    );
    assert_eq!(bytes[1], 0x10, "extended ids, no acknowledgement");
    #[rustfmt::skip]
    assert_eq!(&bytes[8..], &[
        0x00, 0x00,                                     // stream channel 0
        0x00, 0x00,                                     // reserved
        0x00, 0x00, 0x00, 0x02,                         // first packet id
        0x00, 0x00, 0x00, 0x03,                         // last packet id
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, // block id
    ]);
    h.send(&lost);
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame completes")
        .expect("stream error")
        .expect("stream ended");
    assert!(frame.payload.as_ref() == img.as_slice());
    assert_eq!(h.stream.stats().resend_recovered, 2);
}

/// A camera that no longer holds the packets says so with a GVSP packet whose
/// status is `PACKET_UNAVAILABLE` (0x800C): the range is not asked for again,
/// it counts as unavailable, and the frame closes incomplete.
#[tokio::test]
async fn a_range_the_camera_no_longer_holds_is_not_asked_for_again() {
    let (mut h, control) = resend_harness(576, 0);
    h.stream.set_deliver_incomplete(true);
    // Long enough that answering before the first retry is not a race.
    h.stream
        .set_resend_retry_interval(Duration::from_millis(60));
    let script = PacketScript::new().step(ScriptStep::Drop {
        block: 9,
        packet: 2,
    });
    h.send(&script.apply(block_datagrams(
        &BlockSpec::image(9, 64, 32, MONO8, 576),
        &image(64 * 32, 5),
    )));
    let (request, _) = control.next(WAIT).expect("a resend request");
    assert_eq!((request.first, request.last), (2, 2));
    // Header only: status 0x800C, block 9, payload format, packet id 2.
    h.send(&[Datagram {
        block_id: 9,
        packet_id: 2,
        kind: viva_fake_gige::PacketKind::Payload,
        bytes: vec![0x80, 0x0C, 0x00, 0x09, 0x03, 0x00, 0x00, 0x02],
    }]);
    // Four retry intervals: a stream still asking would have asked again.
    assert_eq!(
        control.next(Duration::from_millis(250)),
        None,
        "asked again"
    );

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame closes")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.status, FrameStatus::Incomplete { missing_packets: 1 });
    let stats = h.stream.stats();
    assert_eq!(
        (
            stats.resend_requested,
            stats.resend_recovered,
            stats.resend_unavailable
        ),
        (1, 0, 1),
        "{stats:?}"
    );
}

/// An Allied Vision Manta G-125B answers a request it cannot serve with one
/// header-only `PACKET_UNAVAILABLE` packet per packet asked for (SR-25):
/// three answers for a three-packet range count three packets unavailable,
/// once each, and nothing is asked again. (Ten packets, so a quarter covers
/// the three.)
#[tokio::test]
async fn an_unavailable_answer_per_packet_is_counted_once() {
    let (mut h, control) = resend_harness(576, 0);
    h.stream.set_deliver_incomplete(true);
    h.stream
        .set_resend_retry_interval(Duration::from_millis(60));
    let mut script = PacketScript::new();
    for packet in 2..=4 {
        script = script.step(ScriptStep::Drop { block: 9, packet });
    }
    h.send(&script.apply(block_datagrams(
        &BlockSpec::image(9, 64, 64, MONO8, 576),
        &image(64 * 64, 5),
    )));
    let (request, _) = control.next(WAIT).expect("a resend request");
    assert_eq!((request.first, request.last), (2, 4));
    for packet in 2..=4u8 {
        h.send(&[Datagram {
            block_id: 9,
            packet_id: u32::from(packet),
            kind: viva_fake_gige::PacketKind::Payload,
            bytes: vec![0x80, 0x0C, 0x00, 0x09, 0x03, 0x00, 0x00, packet],
        }]);
    }
    assert_eq!(
        control.next(Duration::from_millis(250)),
        None,
        "asked again"
    );
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame closes")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.status, FrameStatus::Incomplete { missing_packets: 3 });
    let stats = h.stream.stats();
    assert_eq!(
        (
            stats.resend_requested,
            stats.resend_recovered,
            stats.resend_unavailable,
            stats.packets_ignored_error_status
        ),
        (3, 0, 3, 3),
        "{stats:?}"
    );
}

/// A packet-specific refusal in a requested range must not prevent the two
/// neighbours from being requested again and counted as recovered. This
/// models the Manta's per-packet status headers with mixed outcomes.
#[tokio::test]
async fn a_packet_refusal_does_not_cancel_its_neighbours_recovery() {
    let (mut h, control) = resend_harness(576, 0);
    h.stream.set_deliver_incomplete(true);
    h.stream.set_block_timeout(Duration::from_secs(1));
    h.stream
        .set_resend_retry_interval(Duration::from_millis(60));
    let img = image(128 * 64, 19);
    let datagrams = block_datagrams(&BlockSpec::image(9, 128, 64, MONO8, 576), &img);
    let rest: Vec<_> = datagrams
        .iter()
        .filter(|packet| !(2..=4).contains(&packet.packet_id))
        .cloned()
        .collect();
    h.send(&rest);
    let (request, _) = control.next(WAIT).expect("initial range");
    assert_eq!((request.first, request.last), (2, 4));
    let refused = Datagram {
        block_id: 9,
        packet_id: 3,
        kind: viva_fake_gige::PacketKind::Payload,
        bytes: vec![0x80, 0x0C, 0x00, 0x09, 0x03, 0x00, 0x00, 0x03],
    };
    h.send(&[refused.clone(), refused]);
    let (left, _) = control.next(WAIT).expect("retry before refused packet");
    let (right, _) = control.next(WAIT).expect("retry after refused packet");
    assert_eq!((left.first, left.last), (2, 2));
    assert_eq!((right.first, right.last), (4, 4));
    h.send(&[datagrams[2].clone(), datagrams[4].clone()]);
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("incomplete frame deadline")
        .expect("receive")
        .expect("frame");
    assert_eq!(frame.status, FrameStatus::Incomplete { missing_packets: 1 });
    let stats = h.stream.stats();
    assert_eq!(
        (
            stats.resend_requested,
            stats.resend_recovered,
            stats.resend_unavailable
        ),
        (3, 2, 1),
        "{stats:?}"
    );
    let mut expected = img;
    expected[2 * 540..3 * 540].fill(0);
    assert_eq!(
        frame.payload.as_ref(),
        expected.as_slice(),
        "only packet 3 is missing"
    );
}

/// A frame that lost too much is not worth flooding the camera for: at most a
/// quarter of its packets are asked for, retries included.
#[tokio::test]
async fn resend_requests_stop_at_a_quarter_of_the_frame() {
    let (h, control) = resend_harness(576, 0);
    // 40 data packets at a 540-byte stride: 42 packets in all, so 11 may be
    // asked for. Every second data packet is lost: 20 separate ranges.
    let spec = BlockSpec::image(1, 540, 40, MONO8, 576);
    let mut script = PacketScript::new();
    for packet in (2..=40).step_by(2) {
        script = script.step(ScriptStep::Drop { block: 1, packet });
    }
    h.send(&script.apply(block_datagrams(&spec, &image(540 * 40, 6))));

    let mut asked = 0;
    while let Some((request, _)) = control.next(Duration::from_millis(300)) {
        asked += request.last - request.first + 1;
    }
    assert_eq!(asked, 11, "a quarter of 42, rounded up");
    assert_eq!(h.stream.stats().resend_requested, 11);
}

/// An address of this host other than 127.0.0.1: 127.0.0.2 where the whole of
/// 127/8 is local (Linux, Windows), otherwise the first non-loopback IPv4
/// address (macOS, whose `lo0` carries only 127.0.0.1).
fn foreign_source() -> UdpSocket {
    if let Ok(socket) = UdpSocket::bind("127.0.0.2:0") {
        return socket;
    }
    let addr = viva_genicam::gige::Iface::list()
        .expect("interfaces")
        .iter()
        .flat_map(|iface| iface.all_ipv4().unwrap_or_default())
        .find(|ip| !ip.is_loopback())
        .expect("an IPv4 address other than loopback, to send from");
    UdpSocket::bind(SocketAddr::new(IpAddr::V4(addr), 0)).expect("bind foreign source")
}

/// GVSP from any address but the camera's is ignored and counted (SR-03):
/// a stream bound to a port another sender knows is not theirs to fill.
#[tokio::test]
async fn datagrams_from_another_address_are_ignored() {
    let mut h = Harness::new(576);
    h.stream.set_source_filter(Some(Ipv4Addr::LOCALHOST));
    let foreign = foreign_source();
    let intruder = small_block(1);
    for d in &intruder {
        foreign.send_to(&d.bytes, h.dest).expect("send");
    }
    let stats = h.settle(|s| s.packets_ignored_foreign == intruder.len() as u64);
    assert_eq!(
        stats.packets_ignored_foreign,
        intruder.len() as u64,
        "{stats:?}"
    );
    assert!(matches!(
        h.stream.next_block_blocking(Duration::from_millis(200)),
        Err(GenicamError::Timeout(_))
    ));

    h.send(&small_block(2));
    let block = h
        .stream
        .next_block_blocking(WAIT)
        .expect("the camera's own block")
        .expect("stream running");
    assert_eq!(block.block_id(), 2);
}

/// The leader's `padding_x` reaches the frame, and conversion reads each
/// line from its own start (Codex review of #196).
#[tokio::test]
async fn a_leaders_line_padding_reaches_the_frame_and_its_conversion() {
    let mut h = Harness::new(576);
    let mut spec = BlockSpec::image(1, 4, 2, MONO8, 576);
    spec.padding_x = 3;
    let data = [1, 2, 3, 4, 0xEE, 0xEE, 0xEE, 5, 6, 7, 8, 0xEE, 0xEE, 0xEE];
    h.send(&block_datagrams(&spec, &data));
    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("the frame completes")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.padding_x, 3);
    assert_eq!(frame.row_stride(), Some(7));
    let rgb = frame.to_rgb8().expect("convert");
    let grey: Vec<u8> = rgb.as_chunks::<3>().0.iter().map(|px| px[0]).collect();
    assert_eq!(grey, [1, 2, 3, 4, 5, 6, 7, 8]);
}

// ---------------------------------------------------------------------------
// Chunk payloads (TC-06)
// ---------------------------------------------------------------------------

/// An image with chunks (`0x4001`), laid out by the fake's own encoder: the
/// image as the first chunk, then two values, across several data packets.
/// The frame's payload is the image alone; its chunks are kept by their
/// 32-bit IDs.
#[tokio::test]
async fn an_image_with_chunks_arrives_as_its_image_and_its_chunks() {
    let mut h = Harness::new(576);
    let mut spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    spec.payload_type = viva_fake_gige::PAYLOAD_TYPE_IMAGE_EXTENDED_CHUNK;
    let img = image(64 * 32, 4);
    let frame_id = 7u64.to_le_bytes();
    let info = [0, 0, 0, 64, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 8, 0];
    let data = chunk_payload(&[(1, &img), (0x0504_0003, &frame_id), (0xA5A5_0100, &info)]);
    h.send(&block_datagrams(&spec, &data));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("a frame with chunks completes")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload_type, 0x4001);
    assert!(frame.payload.as_ref() == img.as_slice(), "the image alone");
    let chunks = frame.chunks.expect("chunks");
    assert_eq!(
        chunks.ids().collect::<Vec<_>>(),
        vec![1, 0x0504_0003, 0xA5A5_0100]
    );
    assert_eq!(&chunks.get(0x0504_0003).expect("frame id")[..], &frame_id);
    assert_eq!(&chunks.get(0xA5A5_0100).expect("info")[..], &info);
}

/// Chunk data (`0x0004`): a block of chunks with no image and no geometry in
/// its leader. Sized from the packets, as `PayloadSize` is unknown here; the
/// unfixed engine counted it unsupported and delivered nothing.
#[tokio::test]
async fn a_chunk_data_block_is_assembled() {
    let mut h = Harness::new(576);
    let spec = BlockSpec::chunk_data(1, 576);
    let big = image(1500, 9);
    let data = chunk_payload(&[(2, &big), (3, &42u32.to_le_bytes())]);
    h.send(&block_datagrams(&spec, &data));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("a chunk-data block completes")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload_type, 0x0004);
    assert_eq!((frame.width, frame.height), (0, 0));
    assert!(frame.payload.as_ref() == data.as_slice(), "the block whole");
    let chunks = frame.chunks.expect("chunks");
    assert!(chunks.get(2).expect("big")[..] == big[..]);
    assert_eq!(&chunks.get(3).expect("value")[..], &42u32.to_le_bytes());
    assert_eq!(h.stream.stats().frames_unsupported, 0);
}
