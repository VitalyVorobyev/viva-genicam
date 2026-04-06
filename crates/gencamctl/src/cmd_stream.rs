use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use bytes::BytesMut;
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use genicam::gige::gvsp::{self, GvspPacket};
use genicam::pfnc::PixelFormat;
use genicam::{parse_chunk_bytes, Frame, StreamBuilder, StreamDest};

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Debug, Clone)]
pub struct StreamArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<Ipv4Addr>,
    pub mode: String,
    pub group: Option<Ipv4Addr>,
    pub port: u16,
    pub auto: bool,
    pub save: usize,
    pub rgb: bool,
    pub duration_s: u64,
}

struct BlockState {
    block_id: u16,
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    timestamp: u64,
    payload: BytesMut,
}

pub async fn run(args: StreamArgs) -> Result<()> {
    let iface_ip = args
        .iface
        .ok_or_else(|| anyhow!("streaming requires --iface or global --iface"))?;
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, Some(iface_ip), timeout).await?;
    info!(ip = %device.ip, "opening camera for streaming");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for stream")?;
    let mut stream_device = common::open_stream_device(&device)
        .await
        .context("open control channel for stream configuration")?;

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
    if !args.auto {
        builder = builder.auto_packet_size(false);
    }
    if args.port != 0 {
        builder = builder.destination_port(args.port);
    }
    let stream = builder.build().await.context("negotiate stream")?;

    camera.acquisition_start().context("start acquisition")?;
    let mut recv_buffer = vec![0u8; (stream.params().packet_size as usize + 64).max(4096)];
    let stats = stream.stats_handle();
    let mut state: Option<BlockState> = None;
    let mut saved_frames = 0usize;
    let mut frame_index = 0usize;
    let end_deadline = if args.duration_s > 0 {
        Some(Instant::now() + Duration::from_secs(args.duration_s))
    } else {
        None
    };
    let mut interrupted = false;
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let mut ticker = time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if let Some(deadline) = end_deadline {
            if Instant::now() >= deadline {
                info!("stream duration elapsed");
                break;
            }
        }

        tokio::select! {
            _ = ticker.tick() => {
                let snapshot = stream.stats();
                println!(
                    "[stream] fps={:.1} Mbps={:.2} frames={} drops={} resends={}",
                    snapshot.avg_fps,
                    snapshot.avg_mbps,
                    snapshot.frames,
                    snapshot.drops,
                    snapshot.resends,
                );
            }
            _ = &mut ctrl_c => {
                info!("received ctrl-c; stopping stream");
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
                        if let Some(active) = state.as_mut() {
                            if active.block_id == block_id {
                                active.payload.extend_from_slice(data.as_ref());
                            }
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
                        let chunk_map = if chunk_data.is_empty() {
                            None
                        } else {
                            match parse_chunk_bytes(chunk_data.as_ref()) {
                                Ok(map) => Some(map),
                                Err(err) => {
                                    warn!(block_id, error = %err, "failed to decode chunk payload");
                                    None
                                }
                            }
                        };
                        let frame = Frame {
                            payload: active.payload.freeze(),
                            width: active.width,
                            height: active.height,
                            pixel_format: active.pixel_format,
                            chunks: chunk_map,
                            ts_dev: Some(active.timestamp),
                            ts_host: Some(camera.map_dev_ts(active.timestamp)),
                        };
                        frame_index += 1;
                        let latency = frame
                            .host_time()
                            .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                        stats.record_frame(frame.payload.len(), latency);

                        if saved_frames < args.save {
                            if let Err(err) = save_frame(&frame, frame_index, args.rgb) {
                                warn!(error = %err, frame = frame_index, "failed to save frame");
                            } else {
                                saved_frames += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    camera.acquisition_stop().context("stop acquisition")?;
    if interrupted {
        println!("Stream interrupted by user.");
    }
    let summary = stream.stats();
    println!(
        "Summary: frames={} bytes={} drops={} resends={} avg_fps={:.1} avg_mbps={:.2}",
        summary.frames,
        summary.bytes,
        summary.drops,
        summary.resends,
        summary.avg_fps,
        summary.avg_mbps,
    );

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

fn save_frame(frame: &Frame, index: usize, rgb: bool) -> Result<PathBuf> {
    let (buffer, ext) = if !rgb && frame.pixel_format == PixelFormat::Mono8 {
        let data = frame.payload.clone();
        let encoded = common::encode_pgm(frame.width, frame.height, data.as_ref())?;
        (encoded, "pgm")
    } else {
        let rgb_pixels = frame.to_rgb8().context("convert frame to RGB8")?;
        let encoded = common::encode_ppm(frame.width, frame.height, &rgb_pixels)?;
        (encoded, "ppm")
    };
    let path = PathBuf::from(format!("frame_{index:04}.{ext}"));
    common::save_image(&buffer, &path)?;
    info!(file = %path.display(), "saved frame");
    Ok(path)
}
