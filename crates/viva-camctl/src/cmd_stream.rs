use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use viva_genicam::evs::{EvsBlock, EvsFormat};
use viva_genicam::pfnc::PixelFormat;
use viva_genicam::stream::StreamBlock;
use viva_genicam::{Frame, FrameStream, StreamBuilder, StreamDest};

use viva_gige::nic::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Debug, Clone)]
pub struct StreamArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<IfaceSelector>,
    pub mode: String,
    pub group: Option<Ipv4Addr>,
    pub port: u16,
    /// Explicit GVSP packet size ceiling. Mutually exclusive with [`StreamArgs::auto`].
    pub packet_size: Option<u32>,
    /// Set from NIC MTU then path-probe (ADR-0021). Mutually exclusive with
    /// [`StreamArgs::packet_size`].
    pub auto: bool,
    pub save: usize,
    pub rgb: bool,
    pub duration_s: u64,
}

// `await_holding_lock` resolves its lint level at the enclosing coroutine body,
// so a statement-scoped `#[allow]` on the `let stream = { … }` below has no
// effect and the attribute has to live here. The single offending guard is
// documented at the point it is taken; nothing else in this function holds a
// lock across an await.
#[allow(clippy::await_holding_lock)]
pub async fn run(args: StreamArgs) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, args.iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for streaming");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for stream")?;

    let iface = common::resolve_receive_iface(args.iface.as_ref(), device.ip)?;
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
    // Negotiate the stream through the camera's existing GVCP handle so control
    // privilege, stream configuration, and acquisition commands all come from
    // the same application endpoint. This scope also releases the device lock
    // before the higher-level Camera API is used again below.
    //
    // The guard must span the builder's awaits (StreamBuilder borrows the
    // locked device), and nothing else contends this mutex until streaming
    // starts, so holding it across the awaits cannot deadlock here. The
    // `await_holding_lock` allow this needs is on the function; see there.
    let stream = {
        let mut stream_device = camera
            .transport()
            .lock_device()
            .context("access camera control channel for stream configuration")?;
        stream_device
            .claim_control()
            .await
            .context("claim camera control for stream configuration")?;

        let mut builder = StreamBuilder::new(&mut stream_device)
            .iface(iface.clone())
            .dest(dest)
            .rcvbuf_bytes(64 << 20);
        if args.auto {
            builder = builder.auto_packet_size();
        } else if let Some(size) = args.packet_size {
            builder = builder.packet_size(size);
        }
        if args.port != 0 {
            builder = builder.destination_port(args.port);
        }
        builder.build().await.context("negotiate stream")?
    };

    // Keep the CLI at the completed-frame boundary. FrameStream owns the receive
    // buffer, platform-specific packet reception, GVSP reassembly, chunk parsing,
    // and completed-frame statistics.
    let mut frame_stream = FrameStream::new(stream, None);
    // This is a shared snapshot handle; FrameStream records each completed frame
    // exactly once, so the CLI must not call record_frame() again.
    let stats = frame_stream.stats_handle();

    if let Err(err) = camera.set("TLParamsLocked", "1") {
        warn!(error = %err, "failed to lock transport-layer parameters");
    }
    camera.acquisition_start().context("start acquisition")?;
    let mut saved_frames = 0usize;
    let mut frame_index = 0usize;
    let mut warned_evs_rgb = false;
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
        if let Some(deadline) = end_deadline
            && Instant::now() >= deadline
        {
            info!("stream duration elapsed");
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                // No keepalive here: `GigeRegisterIo` runs one for as long as it
                // exists, so the control channel survives a stream that sends no
                // GVCP traffic of its own.
                let snapshot = stats.snapshot();
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
            received = frame_stream.next_block() => {
                match received {
                    Ok(Some(block)) => {
                        match StreamBlock::from(block) {
                            StreamBlock::Image(mut frame) => {
                                // FrameStream has already counted this frame. Mapping its
                                // host timestamp here enriches frame metadata only and
                                // must not trigger another record_frame() call.
                                if let Some(timestamp) = frame.ts_dev {
                                    frame.ts_host = Some(camera.map_dev_ts(timestamp));
                                }
                                frame_index += 1;

                                if saved_frames < args.save {
                                    if let Err(err) = save_frame(&frame, frame_index, args.rgb) {
                                        warn!(error = %err, frame = frame_index, "failed to save frame");
                                    } else {
                                        saved_frames += 1;
                                    }
                                }
                            }
                            StreamBlock::Evs(mut block) => {
                                if let Some(timestamp) = block.ts_dev {
                                    block.ts_host = Some(camera.map_dev_ts(timestamp));
                                }
                                frame_index += 1;

                                if args.rgb && !warned_evs_rgb {
                                    warn!("--rgb does not apply to EVT data; preserving raw bytes");
                                    warned_evs_rgb = true;
                                }
                                if saved_frames < args.save {
                                    if let Err(err) = save_evs_block(&block, frame_index) {
                                        warn!(error = %err, block = frame_index, "failed to save EVT block");
                                    } else {
                                        saved_frames += 1;
                                    }
                                }
                            }
                            _ => warn!("received a GVSP block kind this CLI does not support"),
                        }
                    }
                    Err(err) => {
                        warn!(error = %err, "stream receiver failed");
                        break;
                    }
                    Ok(None) => break,
                }
            }
        }
    }

    camera.acquisition_stop().context("stop acquisition")?;
    if let Err(err) = camera.set("TLParamsLocked", "0") {
        warn!(error = %err, "failed to unlock transport-layer parameters");
    }
    if interrupted {
        println!("Stream interrupted by user.");
    }
    let summary = stats.snapshot();
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

fn save_evs_block(block: &EvsBlock, index: usize) -> Result<PathBuf> {
    let ext = match block.format {
        EvsFormat::Evt30 => "evt3",
        EvsFormat::Evt21 => "evt21",
        _ => "evt",
    };
    let path = PathBuf::from(format!("block_{index:04}.{ext}"));
    common::save_image(block.payload.as_ref(), &path)?;
    info!(
        file = %path.display(),
        format = ?block.format,
        bytes = block.payload.len(),
        "saved raw EVT block"
    );
    Ok(path)
}
