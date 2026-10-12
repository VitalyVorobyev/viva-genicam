//! Integration tests exercising the full Camera API via a fake USB3 Vision
//! transport. No USB hardware is required.
//!
//! Run with: `cargo test -p viva-genicam --features u3v --test fake_u3v_camera`

#![cfg(feature = "u3v")]

use viva_fake_u3v::FakeU3vCamera;
use viva_genicam::open_u3v_device;

#[test]
fn open_fake_u3v_camera() {
    let fake = FakeU3vCamera::new(640, 480);
    let device = fake.open_device().expect("open fake device");
    let (camera, xml) = open_u3v_device(device).expect("open_u3v_device");

    // XML should be valid and parseable.
    assert!(xml.contains("RegisterDescription"));
    assert!(xml.contains("Width"));
    assert!(xml.contains("Height"));

    // Verify ABRM metadata was read correctly.
    let _ = camera; // Camera is usable.
}

#[test]
fn read_features() {
    let fake = FakeU3vCamera::new(640, 480);
    let device = fake.open_device().unwrap();
    let (camera, _xml) = open_u3v_device(device).unwrap();

    let width = camera.get("Width").unwrap();
    assert_eq!(width, "640");

    let height = camera.get("Height").unwrap();
    assert_eq!(height, "480");

    let pixel_format = camera.get("PixelFormat").unwrap();
    assert_eq!(pixel_format, "Mono8");
}

#[test]
fn write_and_readback_feature() {
    let fake = FakeU3vCamera::new(640, 480);
    let device = fake.open_device().unwrap();
    let (mut camera, _xml) = open_u3v_device(device).unwrap();

    camera.set("Width", "320").unwrap();
    let width = camera.get("Width").unwrap();
    assert_eq!(width, "320");

    camera.set("Height", "240").unwrap();
    let height = camera.get("Height").unwrap();
    assert_eq!(height, "240");
}

#[test]
fn stream_frames_from_fake_camera() {
    use std::sync::Arc;
    use viva_fake_u3v::FakeU3vTransport;
    use viva_fake_u3v::registers::REG_ACQ_START;

    let (width, height) = (64u32, 64u32);
    let pixel_format = 0x0108_0001u32; // Mono8

    let transport = Arc::new(FakeU3vTransport::new(width, height, pixel_format));
    let mut device = viva_fake_u3v::open(Arc::clone(&transport)).unwrap();
    let mut stream = device.open_stream(0).expect("SIRM configured");
    device
        .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
        .unwrap();

    let mut last_block = 0;
    for _ in 0..3 {
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.leader.width, width);
        assert_eq!(frame.leader.height, height);
        assert_eq!(frame.leader.pixel_format, pixel_format);
        assert_eq!(frame.payload.len(), (width * height) as usize);
        assert_eq!(frame.trailer.status, 0);
        assert!(frame.leader.block_id > last_block);
        last_block = frame.leader.block_id;
    }
}

#[test]
fn different_pixel_formats() {
    let rgb8 = 0x0218_0014u32; // RGB8Packed
    let fake = FakeU3vCamera::new(320, 240).pixel_format(rgb8);
    let device = fake.open_device().unwrap();
    let (camera, _xml) = open_u3v_device(device).unwrap();

    let pf = camera.get("PixelFormat").unwrap();
    assert_eq!(pf, "RGB8Packed");
}

/// A packed format is 12 bits a pixel, not a whole byte, so frames must not
/// be sized as `width × height × 1`: the reader then takes the rest of the
/// image for the trailer and fails (Codex review of #179). The device's SIRM
/// says how big a block is; 4 608 bytes are not a whole number of the fake's
/// 1 KiB-aligned transfers, so the final transfer carries the rest.
#[tokio::test(flavor = "multi_thread")]
async fn a_packed_format_streams_at_its_payload_size() {
    use viva_genicam::U3vStreamBuilder;

    let device = FakeU3vCamera::new(64, 48).open_device().unwrap();
    let (mut camera, _xml) = open_u3v_device(device).unwrap();
    camera.set("PixelFormat", "Mono12Packed").unwrap();
    assert_eq!(camera.get("PayloadSize").unwrap(), "4608", "64 × 48 × 1.5");

    let mut frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    camera.acquisition_start().unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), frames.next_frame())
        .await
        .expect("no frame within 5 s")
        .expect("stream error")
        .expect("stream ended");
    frames.shutdown().await;

    assert_eq!(frame.payload.len(), 64 * 48 * 3 / 2);
}

/// A fake camera behind a transport the test can inspect.
fn inspectable_camera() -> (
    std::sync::Arc<viva_fake_u3v::FakeU3vTransport>,
    viva_genicam::Camera<viva_genicam::U3vRegisterIo<viva_fake_u3v::FakeU3vTransport>>,
) {
    use std::sync::Arc;
    use viva_fake_u3v::FakeU3vTransport;

    let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
    let device = viva_fake_u3v::open(Arc::clone(&transport)).unwrap();
    let (camera, _xml) = open_u3v_device(device).unwrap();
    (transport, camera)
}

/// Stopping a frame stream waits for its reader thread. `stop()` only set a
/// flag the reader saw after its next USB read, and the thread was detached:
/// a quick restart could leave two readers on one endpoint, and a reconnect
/// find the device still held (Codex review of #179).
#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_a_stream_waits_for_its_reader() {
    use std::sync::Arc;
    use viva_genicam::U3vStreamBuilder;

    let (transport, mut camera) = inspectable_camera();
    let held = Arc::strong_count(&transport);
    let mut frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    camera.acquisition_start().unwrap();
    frames
        .next_frame()
        .await
        .expect("frame")
        .expect("stream ended");

    frames.shutdown().await;

    assert_eq!(
        Arc::strong_count(&transport),
        held,
        "the reader thread still holds the transport"
    );
}

/// The device stops streaming when the host's stream does: stopping clears
/// StreamEnable in the SIRM (Codex review of #181), whether the stream is
/// shut down or dropped.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_stream_clears_stream_enable() {
    use viva_genicam::U3vStreamBuilder;

    let (transport, mut camera) = inspectable_camera();
    let frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    assert!(transport.stream_enabled(), "building a stream enables it");
    frames.shutdown().await;
    assert!(!transport.stream_enabled(), "shutdown leaves it enabled");

    let frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    assert!(transport.stream_enabled());
    drop(frames);
    assert!(!transport.stream_enabled(), "a drop leaves it enabled");
}

/// A camera sends nothing before AcquisitionStart, however the stream is
/// configured.
#[tokio::test(flavor = "multi_thread")]
async fn no_frame_arrives_before_acquisition_starts() {
    use std::time::Duration;
    use viva_genicam::U3vStreamBuilder;

    let (_transport, mut camera) = inspectable_camera();
    let mut frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    assert!(
        tokio::time::timeout(Duration::from_millis(400), frames.next_frame())
            .await
            .is_err(),
        "a frame before AcquisitionStart"
    );

    camera.acquisition_start().unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), frames.next_frame())
        .await
        .expect("no frame within 5 s after AcquisitionStart")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!(frame.payload.len(), 64 * 64);
    frames.shutdown().await;
}

/// A leader and a trailer as Wireshark's `packet-u3v.c` reads them.
fn u3v_block(block_id: u64, status: u16, payload: &[u8]) -> [Vec<u8>; 3] {
    let mut leader = vec![0u8; 52];
    leader[0..4].copy_from_slice(b"U3VL");
    leader[6..8].copy_from_slice(&52u16.to_le_bytes());
    leader[8..16].copy_from_slice(&block_id.to_le_bytes());
    leader[18..20].copy_from_slice(&1u16.to_le_bytes()); // image
    leader[28..32].copy_from_slice(&0x0108_0001u32.to_le_bytes()); // Mono8
    leader[32..36].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    leader[36..40].copy_from_slice(&1u32.to_le_bytes());
    let mut trailer = vec![0u8; 32];
    trailer[0..4].copy_from_slice(b"U3VT");
    trailer[6..8].copy_from_slice(&32u16.to_le_bytes());
    trailer[8..16].copy_from_slice(&block_id.to_le_bytes());
    trailer[16..18].copy_from_slice(&status.to_le_bytes());
    trailer[20..28].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    [leader, payload.to_vec(), trailer]
}

/// A block whose trailer reports an error is dropped and counted, not handed
/// on as an image; a warning status is not an error (GenCP's severity bit,
/// bit 15). Codex review of #184.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_the_device_marks_failed_is_dropped_and_counted() {
    use std::sync::Arc;
    use viva_genicam::U3vFrameStream;
    use viva_u3v::MockUsbTransfer;
    use viva_u3v::{StreamLayout, U3vStream};

    let mock = Arc::new(MockUsbTransfer::new());
    for (id, status) in [(1, 0xA101), (2, 0x0000), (3, 0x2001)] {
        for transfer in u3v_block(id, status, &[id as u8; 64]) {
            mock.enqueue_read(0x82, transfer);
        }
    }
    let layout = StreamLayout::new(64, 52, 32, 4, 1 << 20).unwrap();
    let mut frames = U3vFrameStream::start(U3vStream::new(mock, 0x82, layout));

    let first = frames.next_frame().await.unwrap().expect("a frame");
    assert_eq!(first.payload[0], 2, "the failed block 1 was handed on");
    let second = frames.next_frame().await.unwrap().expect("a frame");
    assert_eq!(second.payload[0], 3, "a warning dropped block 3");
    assert_eq!(frames.dropped(), 1);
    frames.shutdown().await;
}

/// A stream dropped while another thread holds the device still clears
/// StreamEnable once the device is free; it used to give up (Codex review of
/// #184).
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_dropped_while_the_device_is_busy_still_stops() {
    use std::time::{Duration, Instant};
    use viva_genicam::U3vStreamBuilder;

    let (transport, mut camera) = inspectable_camera();
    let frames = U3vStreamBuilder::new(&mut camera).build().expect("stream");
    {
        let _busy = camera.transport().lock_device().unwrap();
        drop(frames);
        assert!(
            transport.stream_enabled(),
            "cleared while the device was locked"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while transport.stream_enabled() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !transport.stream_enabled(),
        "StreamEnable left set after a busy drop"
    );
}
