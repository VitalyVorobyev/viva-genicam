use std::fs::File;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use bytes::BytesMut;
use serde::Serialize;
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use viva_genicam::gige::gvsp::{self, GvspPacket};
use viva_genicam::pfnc::PixelFormat;
use viva_genicam::{Frame, StreamBuilder, StreamDest, parse_chunk_bytes};

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Debug, Clone)]
pub struct BenchArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<Ipv4Addr>,
    pub mode: String,
    pub group: Option<Ipv4Addr>,
    pub port: u16,
    pub duration_s: u64,
    pub json_out: Option<PathBuf>,
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

struct BlockState {
    block_id: u64,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    payload: BytesMut,
}

pub async fn run(args: BenchArgs, emit_json: bool) -> Result<()> {
    let iface_ip = args
        .iface
        .ok_or_else(|| anyhow!("bench requires --iface or global --iface"))?;
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, Some(iface_ip), timeout).await?;
    info!(ip = %device.ip, "opening camera for benchmark");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for bench")?;
    let mut stream_device = common::open_stream_device(&device)
        .await
        .context("open control channel for bench")?;

    let iface = common::resolve_iface(Some(iface_ip))?
        .ok_or_else(|| anyhow!("failed to resolve capture interface"))?;
    let host_ip = iface
        .ipv4()
        .ok_or_else(|| anyhow!("interface {} has no IPv4 address", iface.name()))?;
    let mode = parse_mode(&args.mode)?;

    if let StreamMode::Multicast = mode {
        let group = args
            .group
            .ok_or_else(|| anyhow!("multicast mode requires --group"))?;
        camera
            .configure_stream_multicast(0, group, args.port)
            .context("configure multicast destination")?;
    }

    let mut builder = StreamBuilder::new(&mut stream_device).iface(iface.clone());
    let dest = match mode {
        StreamMode::Unicast => StreamDest::Unicast {
            dst_ip: host_ip,
            dst_port: args.port,
        },
        StreamMode::Multicast => {
            let group = args
                .group
                .ok_or_else(|| anyhow!("multicast mode requires --group"))?;
            StreamDest::Multicast {
                group,
                port: args.port,
                loopback: false,
                ttl: 1,
            }
        }
    };
    builder = builder.dest(dest);
    let stream = builder.build().await.context("negotiate stream")?;

    camera.acquisition_start().context("start acquisition")?;
    let mut recv_buffer = vec![0u8; (stream.params().packet_size as usize + 64).max(4096)];
    let stats = stream.stats_handle();
    let mut state: Option<BlockState> = None;
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let mut ticker = time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let duration = Duration::from_secs(args.duration_s.max(1));
    let end_deadline = Instant::now() + duration;
    let mut interrupted = false;

    loop {
        if Instant::now() >= end_deadline {
            info!("benchmark duration elapsed");
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                let snapshot = stream.stats();
                println!(
                    "[bench] fps={:.1} Mbps={:.2} frames={} drops={} resends={}",
                    snapshot.avg_fps,
                    snapshot.avg_mbps,
                    snapshot.frames,
                    snapshot.drops,
                    snapshot.resends,
                );
            }
            _ = &mut ctrl_c => {
                info!("received ctrl-c; stopping bench early");
                interrupted = true;
                break;
            }
            recv = stream.socket().expect("UDP socket").recv_from(&mut recv_buffer) => {
                let (len, _) = match recv {
                    Ok(result) => result,
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
                    GvspPacket::Leader { block_id, width, height, pixel_format, timestamp, .. } => {
                        state = Some(BlockState {
                            block_id,
                            width,
                            height,
                            pixel_format: PixelFormat::from_code(pixel_format),
                            timestamp,
                            payload: BytesMut::new(),
                        });
                    }
                    GvspPacket::Payload { block_id, data, .. } => {
                        if let Some(active) = state.as_mut()
                            && active.block_id == block_id {
                                active.payload.extend_from_slice(data.as_ref());
                            }
                    }
                    GvspPacket::Trailer { block_id, status, chunk_data, .. } => {
                        let Some(active) = state.take() else { continue };
                        if active.block_id != block_id {
                            continue;
                        }
                        if status != 0 {
                            warn!(block_id, status, "trailer reported non-zero status");
                        }
                        if !chunk_data.is_empty()
                            && let Err(err) = parse_chunk_bytes(chunk_data.as_ref()) {
                                warn!(block_id, error = %err, "failed to decode chunk payload");
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
                    }
                }
            }
        }
    }

    camera.acquisition_stop().context("stop acquisition")?;
    if interrupted {
        println!("Benchmark interrupted by user.");
    }

    let snapshot = stream.stats();
    println!(
        "Summary: frames={} bytes={} drops={} resends={} avg_fps={:.1} avg_mbps={:.2}",
        snapshot.frames,
        snapshot.bytes,
        snapshot.drops,
        snapshot.resends,
        snapshot.avg_fps,
        snapshot.avg_mbps,
    );

    let report = BenchReport {
        duration_s: duration.as_secs(),
        frames: snapshot.frames,
        bytes: snapshot.bytes,
        avg_fps: snapshot.avg_fps,
        avg_mbps: snapshot.avg_mbps,
        drops: snapshot.drops,
        resends: snapshot.resends,
        mode: args.mode.clone(),
    };

    if let Some(path) = args.json_out.as_ref() {
        let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        serde_json::to_writer_pretty(file, &report)
            .with_context(|| format!("write {}", path.display()))?;
        info!(file = %path.display(), "wrote benchmark report");
    }

    if emit_json && args.json_out.is_none() {
        common::print_json(&report)?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamMode {
    Unicast,
    Multicast,
}

fn parse_mode(value: &str) -> Result<StreamMode> {
    match value.to_ascii_lowercase().as_str() {
        "unicast" => Ok(StreamMode::Unicast),
        "multicast" => Ok(StreamMode::Multicast),
        other => bail!("unknown stream mode '{other}' (expected unicast or multicast)"),
    }
}
