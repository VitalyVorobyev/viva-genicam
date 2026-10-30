use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use viva_genicam::{EvsBlock, EvsFormat, StreamBlock};
use viva_genicam::{Frame, ResendPolicy, StreamOptions};

use viva_gige::IfaceSelector;

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
    /// Frames, or EVT blocks, to save as files; `None` is [`save_limit`]'s
    /// default.
    pub save: Option<usize>,
    /// File format of the frames `save` writes.
    pub save_format: common::SaveFormat,
    /// Write every completed EVT block's raw payload to this single file.
    pub raw_out: Option<PathBuf>,
    /// Add to `raw_out` instead of replacing it.
    pub append: bool,
    pub rgb: bool,
    pub duration_s: u64,
    /// How to open the camera. A monitor watches the multicast stream
    /// another application set up ([`StreamOptions::monitor`]) and starts
    /// and stops nothing.
    pub access: viva_genicam::Access,
}

/// Refuse stream settings a monitor cannot make: it watches the stream
/// another application configured, and writes nothing.
pub fn check_monitor_options(
    port: Option<u16>,
    packet_delay: Option<Duration>,
    packet_size: Option<u32>,
    auto: bool,
    mode: &str,
    group: Option<Ipv4Addr>,
) -> Result<()> {
    let multicast = parse_mode(mode)? == StreamMode::Multicast;
    if port.is_some() || packet_delay.is_some() || packet_size.is_some() || auto || multicast {
        bail!(
            "a monitor watches the stream another application configured, reading its group, \
             port and packet size from the camera: pass no --port, --packet-delay-ns, \
             --packet-size, --auto or --mode multicast with it"
        );
    }
    if group.is_some() {
        bail!("a monitor joins the group the camera streams to: pass no --group with it");
    }
    Ok(())
}

pub async fn run(args: StreamArgs) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, args.iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for streaming");
    let monitor = args.access == viva_genicam::Access::Monitor;
    if monitor {
        check_monitor_options(
            args.port,
            args.packet_delay,
            args.packet_size,
            args.auto,
            &args.mode,
            args.group,
        )?;
    }
    let mut camera = common::open_camera(&device, args.access)
        .await
        .context("open camera for stream")?;

    let iface = common::resolve_receive_iface(args.iface.as_ref(), device.ip)?;
    let options = if monitor {
        StreamOptions::monitor().iface(iface)
    } else {
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
        options
    };
    // One recipe for every consumer: claim control, configure the stream
    // channel, negotiate the packet size, and start the receive engine with
    // the camera's PayloadSize. The engine owns reception, reassembly, chunk
    // parsing and the counters. A monitor reads the stream channel instead.
    let mut frame_stream = camera.open_stream(options).context("open stream")?;
    let stats = frame_stream.stats_handle();

    // Sets TLParamsLocked too, and acquisition_stop clears it. A monitor
    // starts and stops nothing: the application that set the stream up does.
    if !monitor {
        camera.acquisition_start().context("start acquisition")?;
    }
    let mut saved_frames = 0usize;
    let mut frame_index = 0usize;
    let mut warned_evs_rgb = false;
    let save_frames = save_limit(args.save, args.raw_out.is_some(), false);
    let save_blocks = save_limit(args.save, args.raw_out.is_some(), true);
    let mut raw_out = args
        .raw_out
        .clone()
        .map(|path| RawOut::new(path, args.append));
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

                                if saved_frames < save_frames {
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
                                    file.write(block.payload.as_ref())
                                        .with_context(|| format!("write EVT payload to raw-out (block {})", frame_index))?;
                                }
                                if saved_frames < save_blocks {
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

    if !monitor {
        camera.acquisition_stop().context("stop acquisition")?;
    }
    if interrupted {
        println!("Stream interrupted by user.");
    }
    common::print_stream_counters("Summary:", &stats.snapshot());

    Ok(())
}

/// How many frames, or EVT blocks, `--save` writes: what it says, else 1 —
/// but no EVT blocks with `--raw-out`, which records every one already.
fn save_limit(save: Option<usize>, raw_out: bool, evs: bool) -> usize {
    save.unwrap_or(if evs && raw_out { 0 } else { 1 })
}

/// The `--raw-out` file, opened when the first EVT block arrives: a stream
/// that sends none leaves no file behind, and an earlier capture as it was.
struct RawOut {
    path: PathBuf,
    append: bool,
    file: Option<std::fs::File>,
}

impl RawOut {
    fn new(path: PathBuf, append: bool) -> Self {
        Self {
            path,
            append,
            file: None,
        }
    }

    /// Write one block's payload, creating the file — or replacing it,
    /// unless appending — before the first.
    fn write(&mut self, payload: &[u8]) -> Result<()> {
        use std::io::Write;
        let file = match self.file.take() {
            Some(file) => file,
            None => {
                let mut options = std::fs::OpenOptions::new();
                options.create(true);
                if self.append {
                    options.append(true);
                } else {
                    options.write(true).truncate(true);
                }
                let file = options
                    .open(&self.path)
                    .with_context(|| format!("open raw-out file {}", self.path.display()))?;
                info!(file = %self.path.display(), append = self.append, "recording EVT blocks");
                file
            }
        };
        self.file.insert(file).write_all(payload)?;
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_defaults_to_one_but_no_evt_block_with_raw_out() {
        assert_eq!(save_limit(None, false, false), 1);
        assert_eq!(save_limit(None, false, true), 1);
        assert_eq!(save_limit(None, true, false), 1, "an image is still saved");
        assert_eq!(save_limit(None, true, true), 0);
        assert_eq!(save_limit(Some(3), true, true), 3, "--save says otherwise");
    }

    #[test]
    fn raw_out_is_created_at_the_first_block_and_replaces_or_appends() {
        let dir = std::env::temp_dir().join(format!("viva-camctl-rawout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.evt3");

        let raw = RawOut::new(path.clone(), false);
        drop(raw);
        assert!(!path.exists(), "no block, no file");

        std::fs::write(&path, b"earlier").unwrap();
        let mut raw = RawOut::new(path.clone(), false);
        raw.write(b"ab").unwrap();
        raw.write(b"cd").unwrap();
        drop(raw);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcd", "replaced");

        let mut raw = RawOut::new(path.clone(), true);
        raw.write(b"ef").unwrap();
        drop(raw);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef", "appended");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
