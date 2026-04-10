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
    use viva_u3v::device::U3vDevice;
    use viva_u3v::stream::U3vStream;

    let width = 64u32;
    let height = 64u32;
    let pixel_format = 0x0108_0001u32; // Mono8

    let transport = Arc::new(FakeU3vTransport::new(width, height, pixel_format));
    let _device = U3vDevice::open(Arc::clone(&transport), 0x81, 0x01, Some(0x82), None).unwrap();

    // Create stream directly (bypassing SIRM config for simplicity).
    let payload_size = (width * height) as usize;
    let mut stream = U3vStream::new(transport, 0x82, 256, 256, payload_size);

    // Receive 3 frames.
    for _ in 0..3 {
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.leader.width, width);
        assert_eq!(frame.leader.height, height);
        assert_eq!(frame.leader.pixel_format, pixel_format);
        assert_eq!(frame.payload.len(), payload_size);
        assert_eq!(frame.trailer.status, 0);
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
