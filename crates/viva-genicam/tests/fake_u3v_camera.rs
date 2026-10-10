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
    use viva_u3v::device::U3vDevice;

    let (width, height) = (64u32, 64u32);
    let pixel_format = 0x0108_0001u32; // Mono8

    let transport = Arc::new(FakeU3vTransport::new(width, height, pixel_format));
    let mut device = U3vDevice::open(Arc::clone(&transport), 0x81, 0x01, Some(0x82), None).unwrap();
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
    use viva_u3v::device::U3vDevice;

    let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
    let device = U3vDevice::open(Arc::clone(&transport), 0x81, 0x01, Some(0x82), None).unwrap();
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
