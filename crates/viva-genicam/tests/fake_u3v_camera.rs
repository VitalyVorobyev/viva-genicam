//! Integration tests exercising the full Camera API via a fake USB3 Vision
//! transport. No USB hardware is required.
//!
//! Run with: `cargo test -p viva-genicam --features u3v --test fake_u3v_camera`

#![cfg(feature = "u3v")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use viva_fake_u3v::registers::{HIGH_SPEED, SIRM_CONTROL};
use viva_fake_u3v::{FakeU3vCamera, FakeU3vTransport, FakeUsbEvent, STREAM_EP_IN};
use viva_genicam::u3v::U3vError;
use viva_genicam::{
    Camera, ChunkConfig, FrameStatus, GenicamError, U3vRegisterIo, U3vStreamBuilder,
    U3vStreamOptions, open_u3v_device,
};

type FakeCamera = Camera<U3vRegisterIo<FakeU3vTransport>>;

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
    use viva_fake_u3v::registers::REG_ACQ_START;

    let (width, height) = (64u32, 64u32);
    let pixel_format = 0x0108_0001u32; // Mono8

    let transport = Arc::new(FakeU3vTransport::new(width, height, pixel_format));
    let mut device = viva_fake_u3v::open(Arc::clone(&transport)).unwrap();
    let mut stream = device.open_stream(0).expect("SIRM configured");
    device
        .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
        .unwrap();

    // The endpoint reset before StreamEnable restarts block ids at 0, as
    // the mvBlueFOX3-2004G does.
    for expected in 0..3 {
        let frame = stream.next_frame().unwrap();
        assert_eq!(frame.leader.width, width);
        assert_eq!(frame.leader.height, height);
        assert_eq!(frame.leader.pixel_format, pixel_format);
        assert_eq!(frame.payload.len(), (width * height) as usize);
        assert_eq!(frame.trailer.expect("a trailer").status, 0);
        assert_eq!(frame.leader.block_id, expected);
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
    let device = FakeU3vCamera::new(64, 48).open_device().unwrap();
    let (mut camera, _xml) = open_u3v_device(device).unwrap();
    camera.set("PixelFormat", "Mono12Packed").unwrap();
    assert_eq!(camera.get("PayloadSize").unwrap(), "4608", "64 × 48 × 1.5");

    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), frames.next_frame())
        .await
        .expect("no frame within 5 s")
        .expect("stream error")
        .expect("stream ended");
    frames.shutdown().await;

    assert_eq!(frame.payload.len(), 64 * 48 * 3 / 2);
}

/// A fake camera of `width` × `height` behind a transport the test can
/// inspect.
fn camera_of(width: u32, height: u32) -> (Arc<FakeU3vTransport>, FakeCamera) {
    let transport = Arc::new(FakeU3vTransport::new(width, height, 0x0108_0001));
    let device = viva_fake_u3v::open(Arc::clone(&transport)).unwrap();
    let (camera, _xml) = open_u3v_device(device).unwrap();
    (transport, camera)
}

fn inspectable_camera() -> (Arc<FakeU3vTransport>, FakeCamera) {
    camera_of(64, 64)
}

/// Stopping a frame stream waits for its reader thread. `stop()` only set a
/// flag the reader saw after its next USB read, and the thread was detached:
/// a quick restart could leave two readers on one endpoint, and a reconnect
/// find the device still held (Codex review of #179).
#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_a_stream_waits_for_its_reader() {
    let (transport, mut camera) = inspectable_camera();
    let held = Arc::strong_count(&transport);
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
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
    let (transport, mut camera) = inspectable_camera();
    let frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    assert!(transport.stream_enabled(), "building a stream enables it");
    frames.shutdown().await;
    assert!(!transport.stream_enabled(), "shutdown leaves it enabled");

    let frames = U3vStreamBuilder::new(camera.transport())
        .build()
        .expect("stream");
    assert!(transport.stream_enabled());
    drop(frames);
    assert!(!transport.stream_enabled(), "a drop leaves it enabled");
}

/// A camera sends nothing before AcquisitionStart, however the stream is
/// configured.
#[tokio::test(flavor = "multi_thread")]
async fn no_frame_arrives_before_acquisition_starts() {
    let (_transport, mut camera) = inspectable_camera();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
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
    assert_eq!(frame.delivered_height, Some(64), "the trailer's Size Y");
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

/// A block whose trailer reports an error is not handed on as an image, and
/// is counted incomplete; a warning status is not an error (GenCP's severity
/// bit, bit 15). Codex review of #184. Asked for, it is delivered with the
/// status it carried.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_the_device_marks_failed_is_dropped_and_counted() {
    use viva_genicam::U3vFrameStream;
    use viva_genicam::u3v::{MockUsbTransfer, StreamLayout, U3vStatus, U3vStream};

    let blocks = || {
        let mock = Arc::new(MockUsbTransfer::new());
        for (id, status) in [(1, 0xA101), (2, 0x0000), (3, 0x2001)] {
            for transfer in u3v_block(id, status, &[id as u8; 64]) {
                mock.enqueue_read(0x82, transfer);
            }
        }
        let layout = StreamLayout::new(64, 52, 32, 4, 1 << 20).unwrap();
        U3vFrameStream::start(U3vStream::open(&mock, 0x82, layout).unwrap())
    };

    let mut frames = blocks();
    let first = frames.next_frame().await.unwrap().expect("a frame");
    assert_eq!(first.payload[0], 2, "the failed block 1 was handed on");
    let second = frames.next_frame().await.unwrap().expect("a frame");
    assert_eq!(second.payload[0], 3, "a warning dropped block 3");
    let stats = frames.stats();
    assert_eq!((stats.frames_complete, stats.frames_incomplete), (2, 1));
    frames.shutdown().await;

    let mut frames = blocks();
    frames.set_deliver_incomplete(true);
    let failed = frames.next_frame().await.unwrap().expect("a frame");
    assert_eq!(
        failed.status,
        FrameStatus::U3vDeviceStatus(U3vStatus::DataOverrun)
    );
    frames.shutdown().await;
}

/// A stream dropped while another thread holds the device still clears
/// StreamEnable once the device is free; it used to give up (Codex review of
/// #184).
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_dropped_while_the_device_is_busy_still_stops() {
    let (transport, mut camera) = inspectable_camera();
    let frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
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

/// Wait up to `within` for `n` complete frames; the frames seen.
fn complete_frames(
    frames: &mut viva_genicam::U3vFrameStream,
    n: usize,
    within: Duration,
) -> Vec<viva_genicam::Frame> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    while seen.len() < n && Instant::now() < deadline {
        match frames.next_frame_blocking(Duration::from_millis(100)) {
            Ok(Some(frame)) => seen.push(frame),
            Ok(None) => break,
            Err(GenicamError::Timeout(_)) => {}
            Err(error) => panic!("stream error: {error}"),
        }
    }
    seen
}

/// UV-01: a camera streaming at full rate overruns unless the host keeps
/// transfers queued; the stream keeps several in flight. The fake drops a
/// block's payload whenever it has fewer than two queued, as a camera's
/// buffer would overflow — one read at a time, the way the stream used to
/// read, loses every block (the negative control).
#[test]
fn queued_transfers_keep_a_full_rate_camera_streaming() {
    let (transport, mut camera) = camera_of(320, 240);
    transport.require_queued_transfers(2);

    let mut one_at_a_time = U3vStreamBuilder::new(camera.transport())
        .transfers_in_flight(1)
        .build()
        .unwrap();
    camera.acquisition_start().unwrap();
    assert!(
        complete_frames(&mut one_at_a_time, 1, Duration::from_millis(500)).is_empty(),
        "a complete frame with one transfer queued"
    );
    let lost = one_at_a_time.stats();
    assert_eq!(lost.frames_complete, 0);
    assert!(lost.frames_incomplete > 0, "{lost:?}");
    one_at_a_time.close();
    camera.acquisition_stop().unwrap();

    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let complete = complete_frames(&mut frames, 5, Duration::from_secs(5));
    assert_eq!(complete.len(), 5, "{:?}", frames.stats());
    assert!(complete.iter().all(|f| f.payload.len() == 320 * 240));
    assert!(
        transport.max_queued_transfers() >= 2,
        "the device saw at most {} queued",
        transport.max_queued_transfers()
    );
    let stats = frames.stats();
    assert!(stats.frames_complete >= 5 && stats.packets_received > 0);
    assert_eq!(stats.bytes, stats.frames_complete * 320 * 240);
    frames.close();
}

/// TC-27 (3): the stream endpoint is reset — halted and cleared — after the
/// transfer layout is written and before StreamEnable is set, as aravis
/// does (`arv_uv_device_reset_stream_endpoint`). The device's own log.
#[test]
fn the_stream_endpoint_is_reset_before_it_is_enabled() {
    let (transport, mut camera) = inspectable_camera();
    let frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    let log = transport.log();
    let reset = log
        .iter()
        .position(|e| {
            *e == FakeUsbEvent::EndpointReset {
                endpoint: STREAM_EP_IN,
            }
        })
        .expect("the stream endpoint was never reset");
    let enable = log
        .iter()
        .position(|e| {
            matches!(e, FakeUsbEvent::Write { address, data }
                if *address == SIRM_CONTROL && data[0] & 1 == 1)
        })
        .expect("StreamEnable was never set");
    let layout = log
        .iter()
        .rposition(|e| {
            matches!(e, FakeUsbEvent::Write { address, .. }
                if (SIRM_CONTROL + 0x14..SIRM_CONTROL + 0x2C).contains(address))
        })
        .expect("no transfer layout written");
    assert!(layout < reset && reset < enable, "{log:?}");
    drop(frames);
}

/// UV-02: a stall mid-block costs that block, not the stream: the endpoint
/// is reset and the stream resynchronises on the next leader. It used to
/// end at the first error.
#[test]
fn a_stall_mid_stream_costs_one_block_not_the_stream() {
    let (transport, mut camera) = camera_of(320, 240);
    // A 76 800-byte payload is 75 packets: halt in the third block.
    transport.stall_stream_after(75 * 2 + 30);
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    frames.set_deliver_incomplete(true);
    let resets = transport.endpoint_resets().len();
    camera.acquisition_start().unwrap();
    let seen = complete_frames(&mut frames, 6, Duration::from_secs(5));
    let stats = frames.stats();
    frames.close();
    let statuses: Vec<bool> = seen.iter().map(|f| f.is_complete()).collect();
    assert_eq!(
        statuses,
        [true, true, false, true, true, true],
        "two blocks, the stalled one, then the stream again"
    );
    assert_eq!(seen[2].status, FrameStatus::TrailerMissing);
    assert_eq!(
        (stats.transfer_errors, stats.frames_incomplete),
        (1, 1),
        "{stats:?}"
    );
    assert_eq!(
        transport.endpoint_resets().len(),
        resets + 1,
        "the device saw no reset after the stall"
    );
}

/// UV-03: a camera unplugged mid-stream ends the stream with a disconnect —
/// its own error, not any USB failure — and the camera's transport says it
/// is lost.
#[test]
fn an_unplugged_camera_ends_its_stream_and_is_lost() {
    let (transport, mut camera) = inspectable_camera();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    assert_eq!(
        complete_frames(&mut frames, 1, Duration::from_secs(5)).len(),
        1
    );
    assert!(!camera.transport().is_lost());

    transport.unplug();
    let deadline = Instant::now() + Duration::from_secs(5);
    let ended = loop {
        match frames.next_frame_blocking(Duration::from_millis(100)) {
            Ok(Some(_)) | Err(GenicamError::Timeout(_)) => {}
            other => break other,
        }
        assert!(Instant::now() < deadline, "the stream never ended");
    };
    assert!(
        matches!(
            ended,
            Err(GenicamError::U3v {
                source: U3vError::Disconnected,
                ..
            })
        ),
        "{ended:?}"
    );
    assert!(matches!(
        frames.next_frame_blocking(Duration::from_millis(10)),
        Ok(None)
    ));
    assert!(camera.transport().is_lost());
    drop(frames);
}

/// UV-03 as macOS reports it — what an mvBlueFOX3-2004G's pulled cable
/// gave: the transfers fail with unknown IOKit codes
/// (`kIOReturnNotResponding`, `kIOReturnNotReady`), never a disconnect, and
/// the device is still listed when the first one fails. The stream still
/// ends with a disconnect once the device is no longer listed, the camera
/// is lost, and stopping acquisition says disconnected too.
#[test]
fn an_unplug_reported_with_unknown_errors_is_still_a_disconnect() {
    let (transport, mut camera) = inspectable_camera();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    assert_eq!(
        complete_frames(&mut frames, 1, Duration::from_secs(5)).len(),
        1
    );

    transport.unplug_as_macos_does(Duration::from_millis(50));
    let deadline = Instant::now() + Duration::from_secs(5);
    let ended = loop {
        match frames.next_frame_blocking(Duration::from_millis(100)) {
            Ok(Some(_)) | Err(GenicamError::Timeout(_)) => {}
            other => break other,
        }
        assert!(Instant::now() < deadline, "the stream never ended");
    };
    assert!(
        matches!(
            ended,
            Err(GenicamError::U3v {
                source: U3vError::Disconnected,
                ..
            })
        ),
        "{ended:?}"
    );
    assert!(camera.transport().is_lost());
    let stop = camera.acquisition_stop().unwrap_err();
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&stop);
    let mut disconnected = false;
    while let Some(error) = cause {
        disconnected |= matches!(
            error.downcast_ref::<U3vError>(),
            Some(U3vError::Disconnected)
        );
        cause = error.source();
    }
    assert!(disconnected, "{stop}");
    drop(frames);
}

/// A feature access on an unplugged camera says it is disconnected, and
/// marks the transport lost.
#[test]
fn a_feature_read_on_an_unplugged_camera_marks_it_lost() {
    let (transport, camera) = inspectable_camera();
    transport.unplug();
    let error = camera.get("Width").unwrap_err();
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    let mut disconnected = false;
    while let Some(e) = cause {
        disconnected |= matches!(e.downcast_ref::<U3vError>(), Some(U3vError::Disconnected));
        cause = e.source();
    }
    assert!(disconnected, "{error:?}");
    assert!(camera.transport().is_lost());
}

/// API-11: chunks arrive in a USB3 Vision frame with little-endian tags and
/// read through the camera's chunk ports: the timestamp chunk is the
/// leader's timestamp, the frame ID chunk its block id.
#[test]
fn chunks_are_read_from_a_u3v_frame() {
    let (_transport, mut camera) = inspectable_camera();
    camera
        .configure_chunks(&ChunkConfig {
            selectors: vec!["Timestamp".into(), "FrameID".into()],
            active: true,
        })
        .unwrap();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let frame = complete_frames(&mut frames, 1, Duration::from_secs(5))
        .pop()
        .expect("a frame");
    frames.close();
    assert_eq!(frame.payload_type, 0x4001);
    assert_eq!(frame.payload.len(), 64 * 64, "the image chunk alone");
    let chunks = frame.chunks.as_ref().expect("chunks");
    assert_eq!(chunks.len(), 3, "image, timestamp, frame id");
    let (timestamp, frame_id) = camera.with_chunks(&frame, |cam| {
        (
            cam.get_integer("ChunkTimestamp").unwrap(),
            cam.get_integer("ChunkFrameID").unwrap(),
        )
    });
    assert_eq!(Some(timestamp as u64), frame.ts_dev);
    assert_eq!(frame_id as u64, frame.block_id);
}

/// TC-27 (4): events arrive from the event interface as GenCP `EVENT_CMD`s,
/// once EventEnable and the event's notification are on, and read through
/// the camera's event port. Dropping the stream clears EventEnable.
#[test]
fn events_arrive_from_the_event_interface() {
    let (transport, mut camera) = inspectable_camera();
    camera.configure_events(&["ExposureEnd"]).unwrap();
    let events = camera.open_event_stream().unwrap();
    assert!(transport.events_enabled());
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let frame = complete_frames(&mut frames, 1, Duration::from_secs(5))
        .pop()
        .expect("a frame");
    let event = loop {
        let event = events
            .next_blocking(Duration::from_secs(5))
            .unwrap()
            .expect("an event");
        let frame_id = camera.with_event(&event, |cam| {
            cam.get_integer("EventExposureEndFrameID").unwrap()
        });
        if frame_id as u64 == frame.block_id {
            break event;
        }
    };
    frames.close();
    assert_eq!(event.id, 0x9001);
    assert_eq!(Some(event.ts_dev), frame.ts_dev);
    let (id, timestamp) = camera.with_event(&event, |cam| {
        (
            cam.get_integer("EventExposureEnd").unwrap(),
            cam.get_integer("EventExposureEndTimestamp").unwrap(),
        )
    });
    assert_eq!((id, timestamp as u64), (0x9001, event.ts_dev));
    drop(events);
    assert!(!transport.events_enabled(), "EventEnable left set");
}

/// Events an earlier event stream left unread on the device are dropped
/// when events are opened again: the event endpoint is reset before
/// EventEnable, as the stream's is. On the mvBlueFOX3-2004G an event from an
/// earlier acquisition came first without it.
#[test]
fn stale_events_are_dropped_when_events_are_opened_again() {
    let (transport, mut camera) = inspectable_camera();
    camera.configure_events(&["ExposureEnd"]).unwrap();
    let first = camera.open_event_stream().unwrap();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    assert_eq!(
        complete_frames(&mut frames, 3, Duration::from_secs(5)).len(),
        3
    );
    camera.acquisition_stop().unwrap();
    frames.close();
    // The reader may have taken some; stop it with events still queued.
    drop(first);

    let resets = transport.endpoint_resets().len();
    let second = camera.open_event_stream().unwrap();
    assert!(
        transport.endpoint_resets()[resets..].contains(&viva_fake_u3v::EVENT_EP_IN),
        "the event endpoint was not reset"
    );
    assert!(
        matches!(
            second.next_blocking(Duration::from_millis(300)),
            Err(GenicamError::Timeout(_))
        ),
        "an event from the earlier acquisition"
    );
}

/// API-11: a USB3 Vision frame carries host time once the camera's clock
/// is fitted — through its timestamp features, here GenCP's bootstrap
/// timestamp — as a GigE Vision frame does.
#[tokio::test(flavor = "multi_thread")]
async fn a_frame_carries_host_time_once_the_clock_is_fitted() {
    let (_transport, mut camera) = inspectable_camera();
    camera.time_calibrate(4, 5).await.unwrap();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let frame = frames.next_frame().await.unwrap().expect("a frame");
    frames.shutdown().await;
    let host = frame.ts_host.expect("no host time");
    let off = std::time::SystemTime::now()
        .duration_since(host)
        .unwrap_or_else(|e| e.duration());
    assert!(off < Duration::from_secs(1), "host time {off:?} away");
}

/// ADR-0027: a program without a Tokio runtime streams from a USB3 Vision
/// camera. The reader used to run on a runtime's blocking pool, which this
/// test does not have.
#[test]
fn the_blocking_api_streams_from_a_u3v_camera() {
    let (_transport, mut camera) = inspectable_camera();
    let mut frames = camera.open_stream(U3vStreamOptions::default()).unwrap();
    camera.acquisition_start().unwrap();
    let frame = frames
        .next_frame_blocking(Duration::from_secs(5))
        .unwrap()
        .expect("a frame");
    assert!(frame.is_complete());
    frames.close();
}

/// A log of what was written, for the USB2 warning.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// UV-03: a camera on a USB 2 link — the SBRM's current speed says high
/// speed — is warned about when it is opened.
#[test]
fn a_camera_below_superspeed_is_warned_about() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let speed = tracing::subscriber::with_default(subscriber, || {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        transport.set_current_speed(HIGH_SPEED);
        let device = viva_fake_u3v::open(transport).unwrap();
        device.current_speed()
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert_eq!(speed, Some(viva_genicam::u3v::UsbSpeed::High));
    assert!(
        log.contains("WARN") && log.contains("below SuperSpeed"),
        "{log}"
    );
}
