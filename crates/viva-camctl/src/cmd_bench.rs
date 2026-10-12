use std::fs::File;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use tokio::time::{self, Instant, MissedTickBehavior};
use tracing::{info, warn};

use viva_genicam::{ResendPolicy, StreamOptions, StreamStats};

use viva_gige::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Debug, Clone)]
pub struct BenchArgs {
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
    pub duration_s: u64,
    pub json_out: Option<PathBuf>,
}

/// The benchmark's result, as the receive engine counted it: every block the
/// camera sent is complete, incomplete, missed or dropped to backpressure.
#[derive(Debug, Serialize)]
struct BenchReport {
    duration_s: u64,
    frames_complete: u64,
    frames_incomplete: u64,
    frames_missed: u64,
    frames_backpressure: u64,
    packets_received: u64,
    packets_missing: u64,
    resend_requested: u64,
    resend_recovered: u64,
    resend_unavailable: u64,
    bytes: u64,
    avg_fps: f64,
    avg_mbps: f64,
    mode: String,
}

impl BenchReport {
    fn new(stats: &StreamStats, duration: Duration, mode: &str) -> Self {
        Self {
            duration_s: duration.as_secs(),
            frames_complete: stats.frames_complete,
            frames_incomplete: stats.frames_incomplete,
            frames_missed: stats.frames_missed,
            frames_backpressure: stats.frames_backpressure,
            packets_received: stats.packets_received,
            packets_missing: stats.packets_missing,
            resend_requested: stats.resend_requested,
            resend_recovered: stats.resend_recovered,
            resend_unavailable: stats.resend_unavailable,
            bytes: stats.bytes,
            avg_fps: stats.avg_fps,
            avg_mbps: stats.avg_mbps,
            mode: mode.to_string(),
        }
    }
}

pub async fn run(args: BenchArgs, emit_json: bool) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(args.ip, args.index, args.iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for benchmark");
    let mut camera = common::open_camera(&device, viva_genicam::Access::Control)
        .await
        .context("open camera for bench")?;

    let iface = common::resolve_receive_iface(args.iface.as_ref(), device.ip)?;
    let mut options = StreamOptions::new().iface(iface).auto_packet_size();
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
    // The receive engine counts every block; the benchmark only reads it.
    let mut frames = camera.open_stream(options).context("open stream")?;

    camera.acquisition_start().context("start acquisition")?;
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
            _ = ticker.tick() => common::print_stream_counters("[bench]", &frames.stats()),
            _ = &mut ctrl_c => {
                info!("received ctrl-c; stopping bench early");
                interrupted = true;
                break;
            }
            block = frames.next_block() => match block {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(err) => {
                    warn!(error = %err, "stream failed");
                    break;
                }
            },
        }
    }

    camera.acquisition_stop().context("stop acquisition")?;
    if interrupted {
        println!("Benchmark interrupted by user.");
    }

    let stats = frames.stats();
    common::print_stream_counters("Summary:", &stats);
    let report = BenchReport::new(&stats, duration, &args.mode);

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
