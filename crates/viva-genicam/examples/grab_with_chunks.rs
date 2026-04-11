use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use tokio::sync::Mutex;
use tracing::{info, warn};
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::gvsp::{self, GvspPacket};
use viva_genicam::gige::nic::Iface;
use viva_genicam::sfnc;
use viva_genicam::{
    Camera, ChunkConfig, ChunkKind, ChunkValue, Frame, GenicamError, GigeRegisterIo, StreamBuilder,
    parse_chunk_bytes,
};
use viva_genicam::{gige::stats::StreamStats, pfnc::PixelFormat};

#[derive(Debug, Default)]
struct Args {
    iface: Option<String>,
}

fn print_usage() {
    eprintln!("usage: grab_with_chunks --iface <name>");
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let name = args
                    .next()
                    .ok_or_else(|| "--iface requires an interface name".to_string())?;
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

#[derive(Debug)]
struct BlockState {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    payload: BytesMut,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let iface_name = match args.iface.as_deref() {
        Some(name) => name,
        None => {
            println!("Please specify the capture interface using --iface <name>.");
            print_usage();
            return Ok(());
        }
    };

    let iface = Iface::from_system(iface_name)?;
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
    let control = std::sync::Arc::new(Mutex::new(
        viva_genicam::gige::GigeDevice::open(control_addr).await?,
    ));
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let control = control.clone();
        move |address, length| {
            let control = control.clone();
            async move {
                let mut dev = control.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(|err| viva_genapi_xml::XmlError::Transport(err.to_string()))
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::from(model);
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

    let mut stream_device = viva_genicam::gige::GigeDevice::open(control_addr).await?;
    let stream = StreamBuilder::new(&mut stream_device)
        .iface(iface.clone())
        .build()
        .await?;

    camera.acquisition_start()?;
    let packet_budget = stream.params().packet_size as usize + 64;
    let mut recv_buffer = vec![0u8; packet_budget.max(4096)];
    let mut frames_remaining = 5usize;
    let mut state: Option<BlockState> = None;
    let mut frame_index = 0usize;
    let stats = stream.stats_handle();
    let mut last_overlay = Instant::now();
    let mut last_pixel_format: Option<PixelFormat> = None;

    while frames_remaining > 0 {
        let (len, _) = stream
            .socket()
            .expect("UDP socket")
            .recv_from(&mut recv_buffer)
            .await?;
        let packet = match gvsp::parse_packet(&recv_buffer[..len]) {
            Ok(packet) => packet,
            Err(err) => {
                warn!(error = %err, "discarding malformed GVSP packet");
                continue;
            }
        };
        match packet {
            GvspPacket::Leader {
                block_id,
                width,
                height,
                pixel_format,
                timestamp,
                ..
            } => {
                let pixel_format = PixelFormat::from_code(pixel_format);
                if last_pixel_format != Some(pixel_format) {
                    info!(
                        block_id,
                        width,
                        height,
                        pixel_format = %pixel_format,
                        "detected pixel format"
                    );
                    last_pixel_format = Some(pixel_format);
                }
                state = Some(BlockState {
                    block_id,
                    width,
                    height,
                    pixel_format,
                    timestamp,
                    payload: BytesMut::new(),
                });
            }
            GvspPacket::Payload { block_id, data, .. } => {
                if let Some(active) = state.as_mut() {
                    if active.block_id == block_id {
                        active.payload.extend_from_slice(data.as_ref());
                    }
                }
            }
            GvspPacket::Trailer {
                block_id,
                status,
                chunk_data,
                ..
            } => {
                let Some(active) = state.take() else { continue };
                if active.block_id != block_id {
                    continue;
                }
                if status != 0 {
                    warn!(block_id, status, "trailer reported non-zero status");
                }
                let chunk_map = match parse_chunk_bytes(chunk_data.as_ref()) {
                    Ok(map) => map,
                    Err(err) => {
                        warn!(block_id, error = %err, "failed to decode chunk payload");
                        HashMap::new()
                    }
                };
                let ts_dev = chunk_map
                    .get(&ChunkKind::Timestamp)
                    .and_then(|value| match value {
                        ChunkValue::U64(ts) => Some(*ts),
                        _ => None,
                    })
                    .or(Some(active.timestamp));
                let ts_host = ts_dev.map(|ticks| camera.map_dev_ts(ticks));
                let frame = Frame {
                    payload: active.payload.freeze(),
                    width: active.width,
                    height: active.height,
                    pixel_format: active.pixel_format,
                    chunks: if chunk_map.is_empty() {
                        None
                    } else {
                        Some(chunk_map)
                    },
                    ts_dev,
                    ts_host,
                };
                let latency = frame
                    .host_time()
                    .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                stats.record_frame(frame.payload.len(), latency);
                frame_index += 1;
                print_frame_summary(frame_index, &frame);
                frames_remaining -= 1;
                if last_overlay.elapsed() >= Duration::from_secs(1) {
                    let snapshot = stats.snapshot();
                    print_overlay(&snapshot);
                    last_overlay = Instant::now();
                }
            }
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
    match frame.chunk(ChunkKind::ExposureTime) {
        Some(ChunkValue::F64(exposure)) => println!("  ExposureTime: {exposure:.3} us"),
        _ => println!("  ExposureTime: <not available>"),
    }
    if let Some(chunks) = frame.chunks.as_ref() {
        for (kind, value) in chunks {
            if matches!(kind, ChunkKind::Timestamp | ChunkKind::ExposureTime) {
                continue;
            }
            match value {
                ChunkValue::U32(bits) => println!("  {kind:?}: 0x{bits:08X}"),
                ChunkValue::U64(value) => println!("  {kind:?}: {value}"),
                ChunkValue::F64(value) => println!("  {kind:?}: {value}"),
                ChunkValue::Bytes(bytes) => println!("  {kind:?}: {} raw bytes", bytes.len()),
            }
        }
    } else {
        println!("  No chunk data reported.");
    }
}

fn print_overlay(stats: &StreamStats) {
    let latency = stats
        .avg_latency_ms
        .map(|ms| format!("{ms:.2} ms"))
        .unwrap_or_else(|| "n/a".to_string());
    println!(
        "[stats] fps={:.1} Mbps={:.2} drops={} resends={} latency={}",
        stats.avg_fps, stats.avg_mbps, stats.drops, stats.resends, latency
    );
}
