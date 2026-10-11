//! GVSP receive engine, driven by scripted datagrams on ephemeral loopback
//! ports. No GVCP, no fake camera process, and nothing binds UDP 3956, so this
//! suite can run beside the fake-camera suites.
//!
//! The datagrams come from `viva_fake_gige::gvsp`, which writes them from the
//! GigE Vision field tables with the fake's own constants (ADR-0019), so the
//! receiver is checked against an independent reading of the wire.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use viva_fake_gige::gvsp::{BlockSpec, Datagram, block_datagrams, image_size, interleave};
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

    /// Send from a thread, `gap` apart, so the receiver runs concurrently.
    fn send_paced(&self, datagrams: Vec<Datagram>, gap: Duration) -> std::thread::JoinHandle<()> {
        let sender = self.sender.try_clone().expect("clone sender");
        let dest = self.dest;
        std::thread::spawn(move || {
            for d in datagrams {
                sender.send_to(&d.bytes, dest).expect("send");
                std::thread::sleep(gap);
            }
        })
    }
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
    h.send(&block_datagrams(&spec, &img, &[]));

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

/// The block timeout counts from the last packet. A frame that takes 300 ms
/// to arrive with 8 ms between packets is healthy; counting from the leader
/// (the 0.8 rule) drops it after 100 ms, and every such frame is lost.
#[tokio::test]
async fn a_slow_transfer_with_small_gaps_completes() {
    let mut h = Harness::new(576);
    // 40 payload packets at a 540-byte stride.
    let spec = BlockSpec::image(1, 216, 100, MONO8, 576);
    let img = image(216 * 100, 2);
    let datagrams = block_datagrams(&spec, &img, &[]);
    assert_eq!(datagrams.len(), 42);
    let sender = h.send_paced(datagrams, Duration::from_millis(8));

    let frame = tokio::time::timeout(WAIT, h.stream.next_frame())
        .await
        .expect("a frame whose packets keep arriving must complete")
        .expect("stream error")
        .expect("stream ended");
    sender.join().unwrap();
    assert!(frame.payload.as_ref() == img.as_slice());
    assert_eq!(frame.status, FrameStatus::Complete);
}

/// A trailer can overtake the last data packet. It says how many packets the
/// block has; it is not a verdict that the block is finished.
#[tokio::test]
async fn a_trailer_before_the_last_payload_completes_the_frame() {
    let mut h = Harness::new(576);
    let spec = BlockSpec::image(1, 64, 32, MONO8, 576);
    let img = image(64 * 32, 3);
    let script = PacketScript::new().step(ScriptStep::TrailerBeforeLastPayload { block: 1 });
    h.send(&script.apply(block_datagrams(&spec, &img, &[])));

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
    h.send(&script.apply(block_datagrams(&spec, &img, &[])));

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
    h.send(&block_datagrams(&spec, &img, &[]));

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
    let first = block_datagrams(&BlockSpec::image(1, 64, 32, MONO8, 576), &a, &[]);
    let second = block_datagrams(&BlockSpec::image(2, 64, 32, MONO8, 576), &b, &[]);
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
        &[],
    )
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
        h.send(&block_datagrams(&spec, &image(1024, 1), &[]));
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
    let datagrams: Vec<Datagram> = (1..=10).flat_map(small_block).collect();
    h.send(&datagrams);
    let stats = h.settle(|s| s.frames_backpressure == 6);
    assert_eq!(stats.frames_backpressure, 6, "{stats:?}");
    let ids: Vec<u64> = (0..4)
        .map(|_| {
            h.stream
                .next_block_blocking(WAIT)
                .expect("a queued block")
                .expect("stream running")
                .block_id()
        })
        .collect();
    assert_eq!(ids, vec![7, 8, 9, 10]);
    let stats = h.stream.stats();
    assert_eq!(stats.frames_complete + stats.frames_backpressure, 10);
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
    h.send(&block_datagrams(&spec, &image(1024, 1), &[]));
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
    h.send(&block_datagrams(&spec, &image(1024, 1), &[]));
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
