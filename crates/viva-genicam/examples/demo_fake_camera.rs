//! Self-contained demo: discover, connect, configure, and stream from a fake
//! GigE Vision camera -- no hardware required.
//!
//! ```bash
//! cargo run -p viva-genicam --example demo_fake_camera
//! ```

use std::time::Duration;

use viva_fake_gige::FakeCamera;
use viva_genicam::gige;
use viva_genicam::{connect_gige_with_xml, Camera, GigeRegisterIo};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // ── 1. Start the fake camera ────────────────────────────────────────────
    println!("Starting fake GigE Vision camera on 127.0.0.1:3956 ...");
    let _camera_guard = FakeCamera::builder()
        .width(640)
        .height(480)
        .fps(10)
        .bind_ip([127, 0, 0, 1].into())
        .port(3956)
        .build()
        .await?;
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
    println!("Connecting to {} ...", dev_info.ip);
    let (camera, xml) = connect_gige_with_xml(dev_info).await?;
    println!(
        "  Connected. GenApi XML: {} bytes, {} features.\n",
        xml.len(),
        camera.nodemap().node_names().count()
    );

    // Wrap for spawn_blocking access
    let camera = std::sync::Arc::new(std::sync::Mutex::new(camera));

    // ── 4. Read camera features ─────────────────────────────────────────────
    println!("Reading camera features:");
    for feature in &[
        "Width",
        "Height",
        "PixelFormat",
        "ExposureTime",
        "Gain",
        "GevTimestampTickFrequency",
    ] {
        let cam = camera.clone();
        let name = feature.to_string();
        let value = tokio::task::spawn_blocking(move || {
            let cam = cam.lock().unwrap();
            cam.get(&name)
        })
        .await?;
        match value {
            Ok(v) => println!("  {feature} = {v}"),
            Err(e) => println!("  {feature} = <error: {e}>"),
        }
    }
    println!();

    // ── 5. Write a feature ──────────────────────────────────────────────────
    println!("Setting Width = 320, ExposureTime = 10000 ...");
    {
        let cam = camera.clone();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam.lock().unwrap();
            cam.set("Width", "320")?;
            cam.set_exposure_time_us(10000.0)?;
            Ok::<_, viva_genicam::GenicamError>(())
        })
        .await??;
    }
    let cam = camera.clone();
    let width = tokio::task::spawn_blocking(move || {
        let cam = cam.lock().unwrap();
        cam.get("Width")
    })
    .await??;
    println!("  Width readback = {width}\n");

    // ── 6. Stream frames ────────────────────────────────────────────────────
    println!("Streaming 5 frames ...");

    // Open a separate control connection for streaming (CCP-holding).
    use std::net::{IpAddr, SocketAddr};
    let control_addr = SocketAddr::new(IpAddr::V4(dev_info.ip), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(control_addr).await?;
    device.claim_control().await?;

    let iface_name = if cfg!(target_os = "macos") {
        "lo0"
    } else {
        "lo"
    };
    let iface = gige::nic::Iface::from_system(iface_name)?;
    let stream = viva_genicam::StreamBuilder::new(&mut device)
        .iface(iface)
        .auto_packet_size(false)
        .build()
        .await?;
    let mut frame_stream = viva_genicam::FrameStream::new(stream, None);

    // Wrap device into Camera for acquisition commands.
    let handle = tokio::runtime::Handle::current();
    let transport = GigeRegisterIo::new(handle, device);
    let nodemap = viva_genicam::genapi::NodeMap::from(viva_genapi_xml::parse(&xml)?);
    let cam = std::sync::Arc::new(std::sync::Mutex::new(Camera::new(transport, nodemap)));

    // Start acquisition via spawn_blocking (block_on can't nest in async).
    let cam2 = cam.clone();
    tokio::task::spawn_blocking(move || {
        cam2.lock().unwrap().acquisition_start().unwrap();
    })
    .await?;

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

        if let Some(ref chunks) = frame.chunks {
            for (kind, value) in chunks.iter() {
                println!("    chunk {:?} = {:?}", kind, value);
            }
        }
    }

    let cam2 = cam.clone();
    tokio::task::spawn_blocking(move || {
        cam2.lock().unwrap().acquisition_stop().unwrap();
    })
    .await?;
    println!("\nDemo complete. All operations succeeded without hardware.");

    Ok(())
}
