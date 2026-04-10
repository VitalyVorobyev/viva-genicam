use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use bytes::BytesMut;
use serde::Serialize;
use tokio::signal;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{info, warn};
use viva_genapi_xml::XmlError;
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::gvsp::{self, GvspPacket};
use viva_genicam::gige::nic::Iface;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::{Camera, Frame, GigeRegisterIo, StreamBuilder, StreamDest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestMode {
    Unicast,
    Multicast,
}

#[derive(Debug, Clone)]
struct Args {
    duration: Duration,
    iface: Ipv4Addr,
    mode: DestMode,
    group: Option<Ipv4Addr>,
    port: Option<u16>,
    ttl: u32,
    loopback: bool,
    stream_idx: u32,
    auto: bool,
    json: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BenchReport {
    duration_s: u64,
    frames: u64,
    bytes: u64,
    avg_fps: f64,
    avg_mbps: f64,
    drops: u64,
    resends: u64,
    mode: String,
}

fn print_usage() {
    eprintln!(
        "usage: soak_bench --duration <Ns> --iface <IPv4> --dest <unicast|multicast> [--group <IPv4> --port <n>] [--ttl <n>] [--loopback] [--stream-idx <n>] [--auto] [--json <path>]"
    );
}

fn parse_duration(text: &str) -> Result<Duration, Box<dyn Error>> {
    if let Some(stripped) = text.strip_suffix('s') {
        let secs: u64 = stripped.parse()?;
        return Ok(Duration::from_secs(secs));
    }
    if let Some(stripped) = text.strip_suffix('m') {
        let mins: u64 = stripped.parse()?;
        return Ok(Duration::from_secs(mins * 60));
    }
    if let Some(stripped) = text.strip_suffix('h') {
        let hours: u64 = stripped.parse()?;
        return Ok(Duration::from_secs(hours * 3600));
    }
    let secs: u64 = text.parse()?;
    Ok(Duration::from_secs(secs))
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut duration = None;
    let mut iface = None;
    let mut mode = None;
    let mut group = None;
    let mut port = None;
    let mut ttl: u32 = 1;
    let mut loopback = false;
    let mut stream_idx = 0u32;
    let mut auto = false;
    let mut json = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--duration" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--duration requires a value".to_string())?;
                duration = Some(parse_duration(&value)?);
            }
            "--iface" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--iface requires an IPv4 address".to_string())?;
                iface = Some(value.parse()?);
            }
            "--dest" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--dest requires a mode".to_string())?;
                mode = Some(match value.as_str() {
                    "unicast" => DestMode::Unicast,
                    "multicast" => DestMode::Multicast,
                    other => {
                        return Err(format!(
                            "unsupported dest mode '{other}'; expected unicast or multicast"
                        )
                        .into())
                    }
                });
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
            "--json" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--json requires a path".to_string())?;
                json = Some(PathBuf::from(value));
            }
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let duration = duration.ok_or_else(|| "--duration is required".to_string())?;
    let iface = iface.ok_or_else(|| "--iface is required".to_string())?;
    let mode = mode.ok_or_else(|| "--dest is required".to_string())?;
    if ttl > 255 {
        return Err("--ttl must be <= 255".into());
    }
    if matches!(mode, DestMode::Multicast) && (group.is_none() || port.is_none()) {
        return Err("multicast mode requires --group and --port".into());
    }

    Ok(Args {
        duration,
        iface,
        mode,
        group,
        port,
        ttl,
        loopback,
        stream_idx,
        auto,
        json,
    })
}

#[derive(Debug)]
struct BlockState {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: viva_genicam::pfnc::PixelFormat,
    timestamp: u64,
    payload: BytesMut,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let iface = Iface::from_ipv4(args.iface)?;

    println!("GigE Vision soak bench");
    println!("  duration: {:?}", args.duration);
    println!("  interface: {} (index {})", iface.name(), iface.index());
    println!("  interface IPv4: {}", args.iface);
    println!("  destination: {:?}", args.mode);
    if let Some(group) = args.group {
        println!("  multicast group: {group}");
    }
    if let Some(port) = args.port {
        println!("  port: {port}");
    }
    println!("  ttl: {}", args.ttl);
    println!("  loopback: {}", if args.loopback { "on" } else { "off" });
    println!("  stream index: {}", args.stream_idx);
    println!(
        "  auto packet negotiation: {}",
        if args.auto { "on" } else { "off" }
    );

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

    if args.mode == DestMode::Multicast {
        camera.configure_stream_multicast(
            args.stream_idx,
            args.group.expect("group required"),
            args.port.expect("port required"),
        )?;
    }

    let mut stream_device = viva_genicam::gige::GigeDevice::open(control_addr).await?;
    let dest = match args.mode {
        DestMode::Unicast => StreamDest::Unicast {
            dst_ip: args.iface,
            dst_port: args.port.unwrap_or(0),
        },
        DestMode::Multicast => StreamDest::Multicast {
            group: args.group.expect("group required"),
            port: args.port.expect("port required"),
            loopback: args.loopback,
            ttl: args.ttl,
        },
    };
    let mut builder = StreamBuilder::new(&mut stream_device)
        .iface(iface.clone())
        .dest(dest)
        .channel(args.stream_idx);
    if !args.auto {
        builder = builder.auto_packet_size(false);
    }
    let stream = builder.build().await?;

    camera.acquisition_start()?;
    let packet_budget = stream.params().packet_size as usize + 64;
    let mut recv_buffer = vec![0u8; packet_budget.max(4096)];
    let stats = stream.stats_handle();
    let mut state: Option<BlockState> = None;
    let mut last_overlay = Instant::now();

    let duration_timer = time::sleep(args.duration);
    tokio::pin!(duration_timer);
    let mut interrupted = false;

    loop {
        tokio::select! {
            _ = &mut duration_timer => {
                info!("duration elapsed; stopping bench");
                break;
            }
            res = signal::ctrl_c() => {
                if let Err(err) = res {
                    warn!(error = %err, "failed to await ctrl-c");
                }
                interrupted = true;
                info!("received ctrl-c; stopping bench");
                break;
            }
            recv = stream.socket().expect("UDP socket").recv_from(&mut recv_buffer) => {
                let (len, _) = match recv {
                    Ok(res) => res,
                    Err(err) => {
                        warn!(error = %err, "socket receive failed");
                        stats.record_drop();
                        continue;
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
                        state = Some(BlockState {
                            block_id,
                            width,
                            height,
                            pixel_format: viva_genicam::pfnc::PixelFormat::from_code(pixel_format),
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
                    GvspPacket::Trailer { block_id, status, .. } => {
                        let Some(active) = state.take() else { continue };
                        if active.block_id != block_id {
                            continue;
                        }
                        if status != 0 {
                            warn!(block_id, status, "trailer reported non-zero status");
                        }
                        let frame = Frame {
                            payload: active.payload.freeze(),
                            width: active.width,
                            height: active.height,
                            pixel_format: active.pixel_format,
                            chunks: None,
                            ts_dev: Some(active.timestamp),
                            ts_host: Some(camera.map_dev_ts(active.timestamp)),
                        };
                        let latency = frame
                            .host_time()
                            .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                        stats.record_frame(frame.payload.len(), latency);

                        if last_overlay.elapsed() >= Duration::from_secs(5) {
                            let snapshot = stats.snapshot();
                            println!(
                                "[soak] fps={:.1} Mbps={:.2} frames={} drops={} resends={}",
                                snapshot.avg_fps,
                                snapshot.avg_mbps,
                                snapshot.frames,
                                snapshot.drops,
                                snapshot.resends
                            );
                            last_overlay = Instant::now();
                        }
                    }
                }
            }
        }
    }

    camera.acquisition_stop()?;
    if interrupted {
        println!("Run interrupted by user; partial results below.");
    }

    let snapshot = stream.stats();
    let report = BenchReport {
        duration_s: snapshot.elapsed.as_secs(),
        frames: snapshot.frames,
        bytes: snapshot.bytes,
        avg_fps: snapshot.avg_fps,
        avg_mbps: snapshot.avg_mbps,
        drops: snapshot.drops,
        resends: snapshot.resends,
        mode: match args.mode {
            DestMode::Unicast => "unicast".to_string(),
            DestMode::Multicast => "multicast".to_string(),
        },
    };

    println!(
        "Summary: frames={} bytes={} avg_fps={:.1} avg_mbps={:.2} drops={} resends={}",
        report.frames, report.bytes, report.avg_fps, report.avg_mbps, report.drops, report.resends
    );

    if let Some(path) = args.json {
        let json = serde_json::to_vec_pretty(&report)?;
        std::fs::write(&path, json)?;
        println!("Wrote report to {}", path.display());
    }

    Ok(())
}
