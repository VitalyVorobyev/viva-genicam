use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use viva_genicam::evs::{EvsBlock, EvsFormat};
use viva_genicam::stream::StreamBlock;
use viva_genicam::{Frame, ResendPolicy, StreamOptions};

use viva_gige::nic::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Debug, Clone)]
pub struct StreamArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<IfaceSelector>,
    pub mode: String,
    pub group: Option<Ipv4Addr>,
    /// Receive port; `None` lets the operating system pick one.
    pub port: Option<u16>,
    /// `GevSCPD` to ask for, in time; `None` leaves it alone.
    pub packet_delay: Option<Duration>,
    /// Ask for lost packets when the camera supports it.
    pub resend: bool,
    /// Explicit GVSP packet size ceiling. Mutually exclusive with [`StreamArgs::auto`].
    pub packet_size: Option<u32>,
    /// Set from NIC MTU then path-probe (ADR-0021). Mutually exclusive with
    /// [`StreamArgs::packet_size`].
    pub auto: bool,
    pub save: usize,
    /// File format of the frames `save` writes.
    pub save_format: common::SaveFormat,
    /// Append every completed EVT block's raw payload to this single file.
    pub raw_out: Option<PathBuf>,
    pub rgb: bool,
    pub duration_s: u64,
}

pub async fn run(args: StreamArgs) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, args.iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for streaming");
    let mut camera = common::open_camera(&device, viva_genicam::Access::Control)
        .await
        .context("open camera for stream")?;

    let iface = common::resolve_receive_iface(args.iface.as_ref(), device.ip)?;
    let mut options = StreamOptions::new().iface(iface);
    if args.auto {
        options = options.auto_packet_size();
    } else if let Some(size) = args.packet_size {
        options = options.packet_size(size);
    }
    if let Some(port) = args.port {
        options = options.destination_port(port);
    }
    if let Some(delay) = args.packet_delay {
        options = options.packet_delay(delay);
    }
    if !args.resend {
        options = options.resend(ResendPolicy::Off);
    }
    if let StreamMode::Multicast = parse_mode(&args.mode)? {
        let group = args
            .group
            .ok_or_else(|| anyhow!("multicast mode requires --group"))?;
        options = options.multicast(group);
    }
    // One recipe for every consumer: claim control, configure the stream
    // channel, negotiate the packet size, and start the receive engine with
    // the camera's PayloadSize. The engine owns reception, reassembly, chunk
    // parsing and the counters.
    let mut frame_stream = camera.open_stream(options).context("open stream")?;
    let stats = frame_stream.stats_handle();

    if let Err(err) = camera.set("TLParamsLocked", "1") {
        warn!(error = %err, "failed to lock transport-layer parameters");
    }
    camera.acquisition_start().context("start acquisition")?;
    let mut saved_frames = 0usize;
    let mut frame_index = 0usize;
    let mut warned_evs_rgb = false;
    let mut raw_out = args
        .raw_out
        .as_ref()
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("open raw-out file {}", path.display()))
        })
        .transpose()?;
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
                common::print_stream_counters("[stream]", &stats.snapshot());
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
                                    frame.ts_host = camera.map_dev_ts(timestamp);
                                }
                                frame_index += 1;

                                if saved_frames < args.save {
                                    if let Err(err) = save_frame(&frame, frame_index, args.save_format, args.rgb) {
                                        warn!(error = %err, frame = frame_index, "failed to save frame");
                                    } else {
                                        saved_frames += 1;
                                    }
                                }
                            }
                            StreamBlock::Evs(mut block) => {
                                if let Some(timestamp) = block.ts_dev {
                                    block.ts_host = camera.map_dev_ts(timestamp);
                                }
                                frame_index += 1;

                                if args.rgb && !warned_evs_rgb {
                                    warn!("--rgb does not apply to EVT data; preserving raw bytes");
                                    warned_evs_rgb = true;
                                }
                                if let Some(file) = raw_out.as_mut() {
                                    use std::io::Write;
                                    file.write_all(block.payload.as_ref())
                                        .with_context(|| format!("write EVT payload to raw-out (block {})", frame_index))?;
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
    common::print_stream_counters("Summary:", &stats.snapshot());

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

fn save_frame(
    frame: &Frame,
    index: usize,
    format: common::SaveFormat,
    rgb: bool,
) -> Result<PathBuf> {
    let (buffer, ext) = common::encode_frame(
        frame.pixel_format,
        frame.width,
        frame.height,
        frame.row_stride(),
        &frame.payload,
        format,
        rgb,
    )?;
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
