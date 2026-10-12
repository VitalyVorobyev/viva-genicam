//! Grab frames with chunk data on and read the chunks through the camera's XML.

use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::sync::Mutex;
use tracing::info;
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::IfaceSelector;
use viva_genicam::sfnc;
use viva_genicam::{Camera, ChunkConfig, Frame, GenicamError, GigeRegisterIo, StreamOptions};
use viva_genicam::{StreamStats, pfnc::PixelFormat};

#[derive(Debug, Default)]
struct Args {
    iface: Option<String>,
}

fn print_usage() {
    eprintln!("usage: grab_with_chunks --iface <HOST-IP|NAME>");
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let name = args
                    .next()
                    .ok_or_else(|| "--iface requires a host IP or interface name".to_string())?;
                parsed.iface = Some(name);
            }
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let selector = match args.iface.as_deref() {
        Some(value) => value.parse::<IfaceSelector>().unwrap(),
        None => {
            println!("Please specify the capture interface using --iface <HOST-IP|NAME>.");
            print_usage();
            return Ok(());
        }
    };

    // Either an IPv4 address or an OS interface name (#109).
    let iface = selector.resolve()?;
    let timeout = Duration::from_millis(500);
    let mut devices = viva_genicam::gige::discover(timeout).await?;
    if devices.is_empty() {
        println!("No GigE Vision devices discovered.");
        return Ok(());
    }
    let device = devices.remove(0);
    println!(
        "Connecting to {} on interface {}",
        device.model.clone().unwrap_or_else(|| "camera".to_string()),
        iface.name()
    );

    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    let control = std::sync::Arc::new(Mutex::new({
        let mut device = viva_genicam::gige::GigeDevice::open(control_addr).await?;
        // The adapter built from it keeps control alive, and releases it
        // when dropped.
        device.claim_control().await?;
        device
    }));
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let control = control.clone();
        move |address, length| {
            let control = control.clone();
            async move {
                let mut dev = control.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(viva_genapi_xml::XmlError::transport)
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::try_from_xml(model)?;
    let handle = tokio::runtime::Handle::current();
    let control_device = match std::sync::Arc::try_unwrap(control) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => return Err("control connection still in use".into()),
    };
    let transport = GigeRegisterIo::new(handle.clone(), control_device);
    let mut camera = Camera::new(transport, nodemap);

    let selectors = match camera.enum_entries(sfnc::CHUNK_SELECTOR) {
        Ok(entries) => entries,
        Err(err) => {
            println!("ChunkSelector enumeration not available: {err}");
            return Ok(());
        }
    };
    let desired = ["Timestamp", "ExposureTime"];
    let mut enable_selectors = Vec::new();
    for wanted in desired {
        if selectors.iter().any(|entry| entry == wanted) {
            enable_selectors.push(wanted.to_string());
        } else {
            println!("Selector '{wanted}' not provided by this camera; skipping.");
        }
    }
    if enable_selectors.is_empty() {
        println!("No compatible chunk selectors available; exiting.");
        return Ok(());
    }

    let cfg = ChunkConfig {
        selectors: enable_selectors.clone(),
        active: true,
    };
    if let Err(err) = camera.configure_chunks(&cfg) {
        match err {
            GenicamError::MissingChunkFeature(name) => {
                println!(
                    "Missing required chunk feature '{name}'. Ensure the camera supports ChunkModeActive."
                );
                return Ok(());
            }
            GenicamError::GenApi(inner) => {
                println!("Failed to enable chunks via GenApi: {inner}");
                return Ok(());
            }
            other => return Err(other.into()),
        }
    }
    println!("Chunk mode enabled for selectors: {:?}", enable_selectors);

    // The receive engine assembles each block and splits off its chunks.
    let mut frames =
        camera.open_stream(StreamOptions::new().iface(iface.clone()).auto_packet_size())?;

    camera.acquisition_start()?;
    let mut frame_index = 0usize;
    let mut last_overlay = Instant::now();
    let mut last_pixel_format: Option<PixelFormat> = None;

    while frame_index < 5 {
        let Some(frame) = frames.next_frame().await? else {
            break;
        };
        if last_pixel_format != Some(frame.pixel_format) {
            info!(
                block_id = frame.block_id,
                width = frame.width,
                height = frame.height,
                pixel_format = %frame.pixel_format,
                "detected pixel format"
            );
            last_pixel_format = Some(frame.pixel_format);
        }
        frame_index += 1;
        print_frame_summary(frame_index, &frame);
        print_chunk_features(&mut camera, &frame);
        if last_overlay.elapsed() >= Duration::from_secs(1) {
            print_overlay(&frames.stats());
            last_overlay = Instant::now();
        }
    }

    camera.acquisition_stop()?;
    println!("Capture complete.");
    Ok(())
}

fn print_frame_summary(index: usize, frame: &Frame) {
    println!("Frame #{index}: {} bytes payload", frame.payload.len());
    println!(
        "  Dimensions: {}x{} ({})",
        frame.width, frame.height, frame.pixel_format
    );
    match frame.ts_dev {
        Some(ts) => println!("  Timestamp (device): {ts}"),
        None => println!("  Timestamp (device): <not available>"),
    }
    match frame.host_time() {
        Some(ts) => match ts.duration_since(UNIX_EPOCH) {
            Ok(duration) => println!(
                "  Timestamp (host): {}.{:09} s",
                duration.as_secs(),
                duration.subsec_nanos()
            ),
            Err(_) => println!("  Timestamp (host): <before UNIX_EPOCH>"),
        },
        None => println!("  Timestamp (host): <not available>"),
    }
}

/// Every chunk feature this frame carries, read through the camera's own XML:
/// which chunk holds a value, at which offset, in which byte order.
fn print_chunk_features(camera: &mut Camera<GigeRegisterIo>, frame: &Frame) {
    if frame.chunks.is_none() {
        println!("  No chunk data reported.");
        return;
    }
    for (name, value) in camera.chunk_features(frame) {
        match value {
            Ok(value) => println!("  {name}: {value}"),
            Err(err) => println!("  {name}: <{err}>"),
        }
    }
    let exposure = camera.with_chunks(frame, |cam| cam.get("ChunkExposureTime"));
    if let Ok(exposure) = exposure {
        println!("  ChunkExposureTime, read alone: {exposure} us");
    }
}

fn print_overlay(stats: &StreamStats) {
    let latency = stats
        .avg_latency_ms
        .map(|ms| format!("{ms:.2} ms"))
        .unwrap_or_else(|| "n/a".to_string());
    println!(
        "[stats] fps={:.1} Mbps={:.2} lost={} latency={}",
        stats.avg_fps,
        stats.avg_mbps,
        stats.frames_lost(),
        latency
    );
}
