//! Self-contained demo: discover, connect, configure, and stream from a fake
//! GigE Vision camera -- no hardware required.
//!
//! ```bash
//! cargo run -p viva-genicam --example demo_fake_camera
//! ```

use std::time::Duration;

use viva_fake_gige::FakeCamera;
use viva_genicam::gige;
use viva_genicam::{ChunkConfig, StreamOptions, connect_gige_with_xml};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // ── 1. Start the fake camera ────────────────────────────────────────────
    println!("Starting fake GigE Vision camera on 127.0.0.1:3956 ...");
    // ANCHOR: fake_camera
    // The guard owns the camera's tasks: it answers GVCP and streams GVSP for
    // as long as it is alive, and shuts down when dropped.
    let _camera_guard = FakeCamera::builder()
        .width(640)
        .height(480)
        .fps(10)
        .bind_ip([127, 0, 0, 1].into())
        .port(3956)
        .build()
        .await?;
    // ANCHOR_END: fake_camera
    println!("  Fake camera is running.\n");

    // ── 2. Discover cameras on the network ──────────────────────────────────
    println!("Discovering cameras (2 s timeout) ...");
    let devices = gige::discover_all(Duration::from_secs(2)).await?;
    println!("  Found {} device(s):", devices.len());
    for dev in &devices {
        println!(
            "    IP: {}  Model: {}  Manufacturer: {}",
            dev.ip,
            dev.model.as_deref().unwrap_or("?"),
            dev.manufacturer.as_deref().unwrap_or("?"),
        );
    }
    println!();

    let dev_info = devices
        .iter()
        .find(|d| d.ip.is_loopback())
        .expect("fake camera not found on loopback");

    // ── 3. Connect and fetch GenApi XML ─────────────────────────────────────
    // ANCHOR: connect
    println!("Connecting to {} ...", dev_info.ip);
    let (mut camera, xml) = connect_gige_with_xml(dev_info).await?;
    println!(
        "  Connected. GenApi XML: {} bytes, {} features.\n",
        xml.len(),
        camera.nodemap().node_names().count()
    );
    // ANCHOR_END: connect

    // ── 4. Read camera features ─────────────────────────────────────────────
    // ANCHOR: read_features
    // `get` and `set` are synchronous. They block on register I/O, and
    // `GigeRegisterIo` steps off the async worker itself, so no `spawn_blocking`
    // wrapper is needed even here inside `#[tokio::main]`.
    println!("Reading camera features:");
    for feature in [
        "Width",
        "Height",
        "PixelFormat",
        "ExposureTime",
        "Gain",
        "GevTimestampTickFrequency",
    ] {
        match camera.get(feature) {
            Ok(value) => println!("  {feature} = {value}"),
            Err(err) => println!("  {feature} = <error: {err}>"),
        }
    }
    println!();

    // ── 5. Write a feature ──────────────────────────────────────────────────
    println!("Setting Width = 320, ExposureTime = 10000 ...");
    camera.set("Width", "320")?;
    camera.set_exposure_time_us(10_000.0)?;
    println!("  Width readback = {}\n", camera.get("Width")?);
    // ANCHOR_END: read_features

    // ── 6. Stream frames, with chunks ───────────────────────────────────────
    println!("Streaming 5 frames with chunk data ...");

    // The camera's own control channel configures the stream.
    let iface_name = if cfg!(target_os = "macos") {
        "lo0"
    } else {
        "lo"
    };
    let iface = gige::nic::Iface::from_system(iface_name)?;
    let mut frame_stream = camera.open_stream(
        StreamOptions::new()
            .iface(iface)
            // Force the multi-packet path: loopback would otherwise hand us a
            // jumbo MTU and a frame this size would fit in a couple of packets.
            .packet_size(1500),
    )?;

    // ANCHOR: enable_chunks
    // Chunk mode: each frame carries the values the camera used for it.
    camera.configure_chunks(&ChunkConfig {
        selectors: vec!["Timestamp".into(), "ExposureTime".into(), "FrameID".into()],
        active: true,
    })?;
    // ANCHOR_END: enable_chunks

    // ── 7. Events ───────────────────────────────────────────────────────────
    // ANCHOR: enable_events
    let host: std::net::Ipv4Addr = [127, 0, 0, 1].into();
    camera
        .configure_events(host, 10_020, &["ExposureEnd"])
        .await?;
    let events = camera.open_event_stream(host, 10_020).await?;
    // ANCHOR_END: enable_events

    camera.acquisition_start()?;

    for i in 0..5 {
        let frame = tokio::time::timeout(Duration::from_secs(5), frame_stream.next_frame())
            .await??
            .expect("stream ended");

        println!(
            "  Frame {}: {}x{} {:?} payload={}B ts={}",
            i + 1,
            frame.width,
            frame.height,
            frame.pixel_format,
            frame.payload.len(),
            frame.ts_dev.unwrap_or(0),
        );

        // ANCHOR: chunk_features
        // Read through the camera's XML: which chunk holds each value, at
        // which offset, in which byte order.
        for (name, value) in camera.chunk_features(&frame) {
            match value {
                Ok(value) => println!("    {name} = {value}"),
                Err(err) => println!("    {name} = <error: {err}>"),
            }
        }
        let exposure = camera.with_chunks(&frame, |cam| cam.get("ChunkExposureTime"))?;
        // ANCHOR_END: chunk_features
        println!("    (ChunkExposureTime alone: {exposure} us)");
    }

    // ANCHOR: event_features
    let event = tokio::time::timeout(Duration::from_secs(5), events.next()).await??;
    println!("  Event 0x{:04X} for block {}:", event.id, event.block_id);
    for (name, value) in camera.event_features(&event) {
        match value {
            Ok(value) => println!("    {name} = {value}"),
            Err(err) => println!("    {name} = <error: {err}>"),
        }
    }
    // ANCHOR_END: event_features

    camera.acquisition_stop()?;
    println!("\nDemo complete. All operations succeeded without hardware.");

    Ok(())
}
