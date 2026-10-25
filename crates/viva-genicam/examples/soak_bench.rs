//! Stream for a long time and report what the receive engine counted, and how
//! evenly frames came: the interval between frames as the application takes
//! them, as their last packets arrived, and between their device timestamps.

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::signal;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{info, warn};
use viva_genapi_xml::XmlError;
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::IfaceSelector;
use viva_genicam::{Camera, FrameStream, GigeRegisterIo, StreamBlock, StreamBuilder, StreamDest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestMode {
    Unicast,
    Multicast,
}

#[derive(Debug, Clone)]
struct Args {
    duration: Duration,
    iface: IfaceSelector,
    mode: DestMode,
    group: Option<Ipv4Addr>,
    port: Option<u16>,
    ttl: u32,
    loopback: bool,
    stream_idx: u32,
    packet_size: Option<u32>,
    json: Option<PathBuf>,
}

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
    bytes: u64,
    avg_fps: f64,
    avg_mbps: f64,
    mode: String,
    /// Interval between frames as the application took them, in µs.
    host_interval_us: Intervals,
    /// Interval between the arrivals of frames' last packets, in µs.
    arrival_interval_us: Intervals,
    /// Interval between the frames' device timestamps, in µs.
    device_interval_us: Intervals,
}

/// Running statistics of the intervals between frames, in microseconds.
#[derive(Debug, Default, Clone, Serialize)]
struct Intervals {
    count: u64,
    mean: f64,
    stddev: f64,
    min: f64,
    max: f64,
    /// Intervals more than 1.5 times the mean: a frame that came late.
    over_1_5x_mean: u64,
    /// The 99th and 99.9th percentiles, from 100 µs bins up to 1 s.
    p99: f64,
    p999: f64,
    #[serde(skip)]
    m2: f64,
    #[serde(skip)]
    bins: Vec<u64>,
}

impl Intervals {
    const BIN_US: f64 = 100.0;
    const BINS: usize = 10_000;

    fn record(&mut self, us: f64) {
        if self.bins.is_empty() {
            self.bins = vec![0; Self::BINS + 1];
            self.min = f64::MAX;
        }
        if self.count > 100 && us > 1.5 * self.mean {
            self.over_1_5x_mean += 1;
        }
        self.count += 1;
        let delta = us - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (us - self.mean);
        self.stddev = (self.m2 / self.count as f64).sqrt();
        self.min = self.min.min(us);
        self.max = self.max.max(us);
        let bin = ((us / Self::BIN_US) as usize).min(Self::BINS);
        self.bins[bin] += 1;
        self.p99 = self.percentile(0.99);
        self.p999 = self.percentile(0.999);
    }

    fn percentile(&self, q: f64) -> f64 {
        let target = (self.count as f64 * q).ceil() as u64;
        let mut seen = 0;
        for (bin, n) in self.bins.iter().enumerate() {
            seen += n;
            if seen >= target {
                return (bin as f64 + 1.0) * Self::BIN_US;
            }
        }
        f64::NAN
    }
}

fn print_usage() {
    eprintln!(
        "usage: soak_bench --duration <Ns> --iface <HOST-IP|NAME> --dest <unicast|multicast> [--group <IPv4> --port <n>] [--ttl <n>] [--loopback] [--stream-idx <n>] [--packet-size <bytes>] [--json <path>]"
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
    let mut packet_size: Option<u32> = None;
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
                    .ok_or_else(|| "--iface requires a host IP or interface name".to_string())?;
                // Either spelling, as everywhere else (#109).
                iface = Some(value.parse::<IfaceSelector>().unwrap());
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
                        .into());
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
            "--packet-size" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--packet-size requires a value in bytes".to_string())?;
                packet_size = Some(value.parse()?);
            }
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
        packet_size,
        json,
    })
}

#[tokio::main]
#[allow(clippy::await_holding_lock)] // the stream is configured under the device lock
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let iface = args.iface.resolve()?;

    println!("GigE Vision soak bench");
    println!("  duration: {:?}", args.duration);
    println!("  interface: {} (index {})", iface.name(), iface.index());
    println!("  interface IPv4: {:?}", iface.ipv4());
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
        "  packet size: {}",
        match args.packet_size {
            Some(size) => format!("{size} (override)"),
            None => "from probed MTU".to_string(),
        }
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
                    .map_err(XmlError::transport)
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::try_from_xml(model)?;
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

    // The stream channel is written over the camera's own control connection:
    // only the application holding control may configure it.
    let mut stream_device = camera.transport().lock_device()?;
    let dest = match args.mode {
        DestMode::Unicast => StreamDest::Unicast {
            // The camera streams *to* the host, so this is the resolved
            // interface's own address — not whatever string `--iface` was
            // spelled with.
            dst_ip: iface
                .ipv4()
                .ok_or_else(|| format!("interface {} has no IPv4 address", iface.name()))?,
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
    if let Some(size) = args.packet_size {
        builder = builder.packet_size(size);
    } else {
        builder = builder.auto_packet_size();
    }
    let stream = builder.build().await?;
    drop(stream_device);

    // The receive engine assembles the blocks and keeps the counters; the
    // benchmark only reads them.
    let mut frames = FrameStream::new(stream, Some(camera.time_sync().clone()));
    camera.acquisition_start()?;
    let mut last_overlay = Instant::now();
    // The device clock's rate, to turn timestamp deltas into microseconds.
    let tick_hz = camera
        .get("GevTimestampTickFrequency")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|hz| *hz > 0.0);
    let mut host_intervals = Intervals::default();
    let mut arrival_intervals = Intervals::default();
    let mut device_intervals = Intervals::default();
    let mut last_host: Option<Instant> = None;
    let mut last_dev: Option<u64> = None;

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
            block = frames.next_block() => {
                match block {
                    Ok(Some(block)) => {
                        let now = Instant::now();
                        if let Some(last) = last_host.replace(now) {
                            host_intervals.record((now - last).as_secs_f64() * 1e6);
                            let arrival = frames.stats().last_frame_dt;
                            if !arrival.is_zero() {
                                arrival_intervals.record(arrival.as_secs_f64() * 1e6);
                            }
                        }
                        if let StreamBlock::Image(frame) = StreamBlock::from(block)
                            && let (Some(ts), Some(hz)) = (frame.ts_dev, tick_hz)
                            && let Some(last) = last_dev.replace(ts)
                        {
                            device_intervals.record(ts.wrapping_sub(last) as f64 / hz * 1e6);
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        warn!(error = %err, "stream failed");
                        break;
                    }
                }
                if last_overlay.elapsed() >= Duration::from_secs(60) {
                    let snapshot = frames.stats();
                    println!(
                        "[soak] t={}s fps={:.2} Mbps={:.2} complete={} incomplete={} missed={} backpressure={} packets_missing={} resend_recovered={} host_interval_us(mean={:.0} sd={:.0} max={:.0} p99.9={:.0} late={}) arrival_interval_us(sd={:.0} max={:.0} late={}) device_interval_us(mean={:.1} sd={:.1} max={:.0})",
                        snapshot.elapsed.as_secs(),
                        snapshot.avg_fps,
                        snapshot.avg_mbps,
                        snapshot.frames_complete,
                        snapshot.frames_incomplete,
                        snapshot.frames_missed,
                        snapshot.frames_backpressure,
                        snapshot.packets_missing,
                        snapshot.resend_recovered,
                        host_intervals.mean,
                        host_intervals.stddev,
                        host_intervals.max,
                        host_intervals.p999,
                        host_intervals.over_1_5x_mean,
                        arrival_intervals.stddev,
                        arrival_intervals.max,
                        arrival_intervals.over_1_5x_mean,
                        device_intervals.mean,
                        device_intervals.stddev,
                        device_intervals.max,
                    );
                    last_overlay = Instant::now();
                }
            }
        }
    }

    camera.acquisition_stop()?;
    if interrupted {
        println!("Run interrupted by user; partial results below.");
    }

    let snapshot = frames.stats();
    let report = BenchReport {
        duration_s: snapshot.elapsed.as_secs(),
        frames_complete: snapshot.frames_complete,
        frames_incomplete: snapshot.frames_incomplete,
        frames_missed: snapshot.frames_missed,
        frames_backpressure: snapshot.frames_backpressure,
        packets_received: snapshot.packets_received,
        packets_missing: snapshot.packets_missing,
        resend_requested: snapshot.resend_requested,
        resend_recovered: snapshot.resend_recovered,
        bytes: snapshot.bytes,
        avg_fps: snapshot.avg_fps,
        avg_mbps: snapshot.avg_mbps,
        mode: match args.mode {
            DestMode::Unicast => "unicast".to_string(),
            DestMode::Multicast => "multicast".to_string(),
        },
        host_interval_us: host_intervals,
        arrival_interval_us: arrival_intervals,
        device_interval_us: device_intervals,
    };

    println!(
        "Summary: complete={} incomplete={} missed={} backpressure={} bytes={} avg_fps={:.1} avg_mbps={:.2}",
        report.frames_complete,
        report.frames_incomplete,
        report.frames_missed,
        report.frames_backpressure,
        report.bytes,
        report.avg_fps,
        report.avg_mbps
    );
    println!(
        "Frame intervals (µs): host {}; arrival {}; device {}",
        serde_json::to_string(&report.host_interval_us)?,
        serde_json::to_string(&report.arrival_interval_us)?,
        serde_json::to_string(&report.device_interval_us)?
    );

    if let Some(path) = args.json {
        let json = serde_json::to_vec_pretty(&report)?;
        std::fs::write(&path, json)?;
        println!("Wrote report to {}", path.display());
    }

    Ok(())
}
