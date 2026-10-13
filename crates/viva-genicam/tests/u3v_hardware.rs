//! A smoke suite for a real USB3 Vision camera. Every test is ignored: it
//! needs a camera plugged in, which CI does not have.
//!
//! ```bash
//! cargo test -p viva-genicam --features u3v-usb --test u3v_hardware -- --ignored --test-threads=1
//! ```
//!
//! One test at a time: a USB device admits one owner. The suite uses the
//! first camera `list-usb` shows, or `VIVA_U3V_INDEX`. It turns the
//! FrameStart trigger off while it streams, and chunk mode and events on
//! and off, putting back what it found; it writes nothing non-volatile.
//! Written against a MATRIX VISION mvBlueFOX3-2004G; feature names other
//! cameras lack are skipped with a message.

#![cfg(feature = "u3v-usb")]

use std::time::{Duration, Instant, SystemTime};

use viva_genicam::u3v::{NusbTransfer, U3vDeviceInfo, discover};
use viva_genicam::{
    Camera, ChunkConfig, Frame, GenicamError, U3vFrameStream, U3vRegisterIo, U3vStreamOptions,
    connect_u3v,
};

type UsbCamera = Camera<U3vRegisterIo<NusbTransfer>>;

const NEEDS: &str = "needs a USB3 Vision camera";

fn device() -> U3vDeviceInfo {
    let index: usize = std::env::var("VIVA_U3V_INDEX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    discover()
        .expect("USB enumeration")
        .into_iter()
        .nth(index)
        .unwrap_or_else(|| panic!("no USB3 Vision camera at index {index}"))
}

/// The camera, free-running: FrameStart's trigger off. Returns what the
/// trigger was, to put back.
fn camera() -> (UsbCamera, Option<String>) {
    let mut camera = connect_u3v(&device()).expect("connect");
    let trigger = camera
        .set("TriggerSelector", "FrameStart")
        .and_then(|()| camera.get("TriggerMode"))
        .ok();
    if trigger.is_some() {
        camera.set("TriggerMode", "Off").expect("TriggerMode Off");
    }
    (camera, trigger)
}

fn restore(mut camera: UsbCamera, trigger: Option<String>) {
    if let Some(mode) = trigger {
        let _ = camera.set("TriggerSelector", "FrameStart");
        let _ = camera.set("TriggerMode", &mode);
    }
}

/// `n` frames, each within `timeout`.
fn frames(stream: &mut U3vFrameStream, n: usize, timeout: Duration) -> Vec<Frame> {
    (0..n)
        .map(|i| match stream.next_frame_blocking(timeout) {
            Ok(Some(frame)) => frame,
            Ok(None) => panic!("the stream ended after {i} frames"),
            Err(error) => panic!("frame {i}: {error}"),
        })
        .collect()
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn discover_lists_the_camera_from_the_os_cache() {
    let found = discover().expect("USB enumeration");
    assert!(!found.is_empty(), "{NEEDS}");
    for info in &found {
        println!("{info}: serial {:?}, speed {:?}", info.serial, info.speed);
        assert!(info.speed.is_some(), "the OS reports a speed");
    }
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn open_and_read_features() {
    let camera = connect_u3v(&device()).expect("connect");
    let model = camera.get("DeviceModelName").expect("DeviceModelName");
    let width = camera.get_integer("Width").expect("Width");
    let payload = camera.get_integer("PayloadSize").expect("PayloadSize");
    println!("{model}: Width {width}, PayloadSize {payload}");
    assert!(!model.is_empty() && width > 0 && payload > 0);
    let device = camera.transport().lock_device().expect("device");
    println!(
        "GenCP {:#x}, U3V {:#x}, speed {:?}",
        device.abrm().gencp_version,
        device.sbrm().u3v_version,
        device.current_speed()
    );
    assert!(!camera.transport().is_lost());
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn a_stream_delivers_frames_with_valid_data() {
    let (mut camera, trigger) = camera();
    let payload = camera.get_integer("PayloadSize").expect("PayloadSize") as usize;
    let mut stream = camera
        .open_stream(U3vStreamOptions::default())
        .expect("stream");
    camera.acquisition_start().expect("start");
    let started = Instant::now();
    let got = frames(&mut stream, 30, Duration::from_secs(5));
    let elapsed = started.elapsed();
    camera.acquisition_stop().expect("stop");
    let stats = stream.stats();
    stream.close();
    restore(camera, trigger);

    for pair in got.windows(2) {
        assert_eq!(pair[1].block_id, pair[0].block_id + 1, "consecutive ids");
        assert!(pair[1].ts_dev > pair[0].ts_dev, "timestamps increase");
    }
    for frame in &got {
        assert!(frame.is_complete());
        // The whole block; with chunk mode on, `payload` is the image alone.
        assert_eq!(frame.raw_payload.len(), payload);
        assert_eq!(frame.delivered_height, Some(frame.height));
    }
    let pixels = &got[0].payload;
    let (min, max) = (pixels.iter().min(), pixels.iter().max());
    println!(
        "30 frames in {elapsed:?}; block ids {}..{}; pixel values {min:?}..{max:?}; {stats:?}",
        got[0].block_id, got[29].block_id
    );
    assert!(min < max, "a flat frame: is the lens covered?");
    assert_eq!(stats.transfer_errors, 0);
    assert_eq!(stats.packets_ignored_malformed, 0);
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn chunks_carry_the_frames_values() {
    let (mut camera, trigger) = camera();
    let width = camera.get_integer("Width").expect("Width");
    let exposure = camera.get_float("ExposureTime").ok();
    let mut selectors = vec!["Timestamp".to_string(), "Width".to_string()];
    if exposure.is_some() {
        selectors.push("ExposureTime".to_string());
    }
    camera
        .configure_chunks(&ChunkConfig {
            selectors: selectors.clone(),
            active: true,
        })
        .expect("chunk mode");
    let mut stream = camera
        .open_stream(U3vStreamOptions::default())
        .expect("stream");
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 3, Duration::from_secs(5));
    camera.acquisition_stop().expect("stop");
    stream.close();
    for frame in &got {
        let chunks = frame.chunks.as_ref().expect("the frame carries chunks");
        println!(
            "block {}: payload type {:#06x}, chunk ids {:x?}",
            frame.block_id,
            frame.payload_type,
            chunks.ids().collect::<Vec<_>>()
        );
        let (timestamp, chunk_width, chunk_exposure) = camera.with_chunks(frame, |cam| {
            (
                cam.get_integer("ChunkTimestamp").expect("ChunkTimestamp"),
                cam.get_integer("ChunkWidth").expect("ChunkWidth"),
                cam.get_float("ChunkExposureTime").ok(),
            )
        });
        assert_eq!(Some(timestamp as u64), frame.ts_dev, "chunk = leader time");
        assert_eq!(chunk_width, width);
        if let (Some(feature), Some(chunk)) = (exposure, chunk_exposure) {
            assert!((feature - chunk).abs() < 1.0, "{feature} vs {chunk}");
        }
        assert_eq!(frame.payload.len(), (frame.width * frame.height) as usize);
    }
    camera
        .configure_chunks(&ChunkConfig {
            selectors,
            active: false,
        })
        .expect("chunk mode off");
    restore(camera, trigger);
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn events_arrive_with_their_frames() {
    let (mut camera, trigger) = camera();
    camera
        .configure_events(&["ExposureEnd"])
        .expect("EventSelector");
    let events = camera.open_event_stream().expect("event stream");
    let mut stream = camera
        .open_stream(U3vStreamOptions::default())
        .expect("stream");
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 5, Duration::from_secs(5));
    let mut matched = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while matched < 3 && Instant::now() < deadline {
        let event = match events.next_blocking(Duration::from_millis(500)) {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(GenicamError::Timeout(_)) => continue,
            Err(error) => panic!("events: {error}"),
        };
        // Whichever event this is — another may have been left on — its
        // frame ID feature.
        let Some(frame_id) = camera
            .event_features(&event)
            .into_iter()
            .find(|(name, _)| name.ends_with("FrameID"))
            .and_then(|(_, value)| value.ok()?.parse::<i64>().ok())
        else {
            continue;
        };
        if let Some(frame) = got.iter().find(|f| f.block_id == frame_id as u64) {
            let frame_time = frame.ts_dev.expect("frame time") as i128;
            let gap = event.ts_dev as i128 - frame_time;
            println!(
                "event {:#06x} for block {frame_id}: {gap} ns from the frame's timestamp",
                event.id
            );
            // One clock: the event is within a second of its frame.
            assert!(gap.abs() < 1_000_000_000, "{gap} ns apart");
            matched += 1;
        }
    }
    camera.acquisition_stop().expect("stop");
    stream.close();
    drop(events);
    for entry in camera.enum_entries("EventSelector").unwrap_or_default() {
        let _ = camera.set("EventSelector", &entry);
        let _ = camera.set("EventNotification", "Off");
    }
    restore(camera, trigger);
    assert!(matched >= 3, "events for the frames streamed: {matched}");
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn a_stream_stops_and_starts_again() {
    let (mut camera, trigger) = camera();
    for round in 0..3 {
        let mut stream = camera
            .open_stream(U3vStreamOptions::default())
            .expect("stream");
        camera.acquisition_start().expect("start");
        let got = frames(&mut stream, 5, Duration::from_secs(5));
        camera.acquisition_stop().expect("stop");
        stream.close();
        let enable = {
            let mut device = camera.transport().lock_device().expect("device");
            let sirm = device.read_sirm().expect("SIRM");
            sirm.control
        };
        println!(
            "round {round}: blocks {}..{}, SIRM control {enable:#x} after close",
            got[0].block_id, got[4].block_id
        );
        assert_eq!(enable & 1, 0, "StreamEnable left set");
    }
    restore(camera, trigger);
}

#[test]
#[ignore = "needs a USB3 Vision camera"]
fn a_fitted_clock_gives_frames_host_time() {
    let (mut camera, trigger) = camera();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    runtime
        .block_on(camera.time_calibrate(8, 10))
        .expect("time_calibrate");
    let mut stream = camera
        .open_stream(U3vStreamOptions::default())
        .expect("stream");
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 3, Duration::from_secs(5));
    camera.acquisition_stop().expect("stop");
    stream.close();
    restore(camera, trigger);
    for frame in got {
        let host = frame.ts_host.expect("host time");
        let off = SystemTime::now()
            .duration_since(host)
            .unwrap_or_else(|e| e.duration());
        println!("block {}: host time {off:?} ago", frame.block_id);
        assert!(off < Duration::from_secs(2));
    }
}
