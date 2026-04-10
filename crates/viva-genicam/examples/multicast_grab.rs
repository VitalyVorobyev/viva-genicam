use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use tokio::sync::Mutex;
use tracing::{info, warn};
use viva_genapi_xml::XmlError;
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::gvsp::{self, GvspPacket};
use viva_genicam::gige::nic::Iface;
use viva_genicam::gige::stats::StreamStats;
use viva_genicam::pfnc::PixelFormat;
use viva_genicam::{Camera, Frame, GigeRegisterIo, StreamBuilder, StreamDest};

#[derive(Debug, Clone)]
struct Args {
    iface: Ipv4Addr,
    group: Ipv4Addr,
    port: u16,
    ttl: u32,
    loopback: bool,
    stream_idx: u32,
    auto: bool,
    save: usize,
}

fn print_usage() {
    eprintln!(
        "usage: multicast_grab --iface <IPv4> --group <IPv4> --port <n> [--ttl <n>] [--loopback] [--stream-idx <n>] [--auto] [--save <n>]"
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut iface = None;
    let mut group = None;
    let mut port = None;
    let mut ttl: u32 = 1;
    let mut loopback = false;
    let mut stream_idx = 0u32;
    let mut auto = false;
    let mut save = 0usize;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--iface requires an IPv4 address".to_string())?;
                iface = Some(value.parse()?);
            }
            "--group" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--group requires an IPv4 address".to_string())?;
                group = Some(value.parse()?);
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port requires a value".to_string())?;
                port = Some(value.parse()?);
            }
            "--ttl" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--ttl requires a value".to_string())?;
                ttl = value.parse()?;
            }
            "--loopback" => loopback = true,
            "--stream-idx" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--stream-idx requires a value".to_string())?;
                stream_idx = value.parse()?;
            }
            "--auto" => auto = true,
            "--save" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--save requires a count".to_string())?;
                save = value.parse()?;
            }
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let iface = iface.ok_or_else(|| "--iface is required".to_string())?;
    let group = group.ok_or_else(|| "--group is required".to_string())?;
    let port = port.ok_or_else(|| "--port is required".to_string())?;
    if ttl > 255 {
        return Err("--ttl must be <= 255".into());
    }

    Ok(Args {
        iface,
        group,
        port,
        ttl,
        loopback,
        stream_idx,
        auto,
        save,
    })
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
    let iface = Iface::from_ipv4(args.iface)?;

    println!("GigE Vision multicast capture");
    println!("  interface: {} (index {})", iface.name(), iface.index());
    println!("  interface IPv4: {}", args.iface);
    println!("  multicast group: {}", args.group);
    println!("  port: {}", args.port);
    println!("  ttl: {}", args.ttl);
    println!("  loopback: {}", if args.loopback { "on" } else { "off" });
    println!("  stream index: {}", args.stream_idx);
    println!(
        "  auto packet negotiation: {}",
        if args.auto { "on" } else { "off" }
    );
    println!("  save frames: {}", args.save);

    let timeout = Duration::from_millis(500);
    let mut devices = viva_genicam::gige::discover(timeout).await?;
    if devices.is_empty() {
        println!("No GigE Vision devices discovered.");
        return Ok(());
    }
    let device = devices.remove(0);
    println!(
        "  using device: {} @ {}",
        device.model.clone().unwrap_or_else(|| "camera".to_string()),
        device.ip
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
                    .map_err(|err| XmlError::Transport(err.to_string()))
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::from(model);
    let handle = tokio::runtime::Handle::current();
    let control_device = std::sync::Arc::try_unwrap(control)
        .map_err(|_| "control connection still in use")?
        .into_inner();
    let transport = GigeRegisterIo::new(handle.clone(), control_device);
    let mut camera = Camera::new(transport, nodemap);

    camera.configure_stream_multicast(args.stream_idx, args.group, args.port)?;

    let mut stream_device = viva_genicam::gige::GigeDevice::open(control_addr).await?;
    let mut builder = StreamBuilder::new(&mut stream_device)
        .iface(iface.clone())
        .dest(StreamDest::Multicast {
            group: args.group,
            port: args.port,
            loopback: args.loopback,
            ttl: args.ttl,
        })
        .channel(args.stream_idx);
    if !args.auto {
        builder = builder.auto_packet_size(false);
    }
    let stream = builder.build().await?;

    camera.acquisition_start()?;
    let packet_budget = stream.params().packet_size as usize + 64;
    let mut recv_buffer = vec![0u8; packet_budget.max(4096)];
    let mut state: Option<BlockState> = None;
    let stats = stream.stats_handle();
    let mut last_overlay = Instant::now();
    let mut last_pixel_format: Option<PixelFormat> = None;
    let mut frame_index = 0usize;
    let mut save_remaining = args.save;

    println!("Joined multicast group; waiting for frames...");

    loop {
        let (len, _) = match stream
            .socket()
            .expect("UDP socket")
            .recv_from(&mut recv_buffer)
            .await
        {
            Ok(res) => res,
            Err(err) => {
                warn!(error = %err, "socket receive failed; stopping capture");
                break;
            }
        };
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
                block_id, status, ..
            } => {
                let Some(active) = state.take() else { continue };
                if active.block_id != block_id {
                    continue;
                }
                if status != 0 {
                    warn!(block_id, status, "trailer reported non-zero status");
                }
                let ts_dev = Some(active.timestamp);
                let ts_host = ts_dev.map(|ticks| camera.map_dev_ts(ticks));
                let frame = Frame {
                    payload: active.payload.freeze(),
                    width: active.width,
                    height: active.height,
                    pixel_format: active.pixel_format,
                    chunks: None,
                    ts_dev,
                    ts_host,
                };
                let latency = frame
                    .host_time()
                    .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                stats.record_frame(frame.payload.len(), latency);
                frame_index += 1;
                print_frame_info(frame_index, &frame);

                if save_remaining > 0 {
                    match save_frame(&frame, frame_index) {
                        Ok(path) => println!("  saved {}", path.display()),
                        Err(err) => warn!(error = %err, "failed to save frame"),
                    }
                    save_remaining = save_remaining.saturating_sub(1);
                }

                if last_overlay.elapsed() >= Duration::from_secs(1) {
                    let snapshot = stats.snapshot();
                    print_overlay(&snapshot);
                    last_overlay = Instant::now();
                }
            }
        }
    }

    camera.acquisition_stop()?;
    println!("Capture stopped.");
    Ok(())
}

fn save_frame(frame: &Frame, index: usize) -> Result<PathBuf, Box<dyn Error>> {
    let width =
        usize::try_from(frame.width).map_err(|_| "frame width exceeds host address space")?;
    let height =
        usize::try_from(frame.height).map_err(|_| "frame height exceeds host address space")?;
    let stem = format!("frame_{index:03}");

    if frame.pixel_format != PixelFormat::Mono8 {
        let rgb = frame
            .to_rgb8()
            .map_err(|err| -> Box<dyn Error> { Box::new(err) })?;
        let path = PathBuf::from(format!("{stem}.ppm"));
        write_ppm(path.clone(), width, height, &rgb)?;
        Ok(path)
    } else {
        let path = PathBuf::from(format!("{stem}.pgm"));
        write_pgm(path.clone(), width, height, frame.payload.as_ref())?;
        Ok(path)
    }
}

fn write_pgm(
    path: PathBuf,
    width: usize,
    height: usize,
    data: &[u8],
) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(path)?;
    writeln!(file, "P5\n{} {}\n255", width, height)?;
    file.write_all(data)?;
    Ok(())
}

fn write_ppm(
    path: PathBuf,
    width: usize,
    height: usize,
    data: &[u8],
) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(path)?;
    writeln!(file, "P6\n{} {}\n255", width, height)?;
    file.write_all(data)?;
    Ok(())
}

fn print_frame_info(index: usize, frame: &Frame) {
    println!(
        "Frame #{index}: {} bytes {}x{} {}",
        frame.payload.len(),
        frame.width,
        frame.height,
        frame.pixel_format
    );
    match frame.host_time() {
        Some(ts) => match ts.duration_since(UNIX_EPOCH) {
            Ok(duration) => println!(
                "  host ts: {}.{:09} s",
                duration.as_secs(),
                duration.subsec_nanos()
            ),
            Err(_) => println!("  host ts: <before UNIX_EPOCH>"),
        },
        None => println!("  host ts: <not available>"),
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
