//! Simplified GigE Vision frame grabber using the high-level FrameStream API.
//!
//! Demonstrates the ergonomic streaming interface that handles packet reassembly
//! automatically, reducing the main acquisition loop to just a few lines.

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant, UNIX_EPOCH};

use tracing::warn;
use viva_genicam::gige::nic::Iface;
use viva_genicam::gige::stats::StreamStats;
use viva_genicam::pfnc::PixelFormat;
use viva_genicam::{Frame, FrameStream, StreamBuilder, connect_gige};

#[derive(Debug, Clone)]
struct Args {
    iface: Option<Iface>,
    auto: bool,
    multicast: Option<Ipv4Addr>,
    port: Option<u16>,
    save: usize,
    rgb: bool,
}

fn print_usage() {
    eprintln!(
        "usage: grab_gige --iface <name> [--auto] [--multicast <ip>] [--port <n>] [--save <n>] [--rgb]"
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut iface = None;
    let mut auto = false;
    let mut multicast = None;
    let mut port = None;
    let mut save = 1usize;
    let mut rgb = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let name = args
                    .next()
                    .ok_or_else(|| "--iface requires an argument".to_string())?;
                iface = Some(Iface::from_system(&name)?);
            }
            "--auto" => auto = true,
            "--multicast" => {
                let ip = args
                    .next()
                    .ok_or_else(|| "--multicast requires an IPv4 address".to_string())?;
                multicast = Some(ip.parse()?);
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port requires a value".to_string())?;
                port = Some(value.parse()?);
            }
            "--save" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--save requires a frame count".to_string())?;
                save = value.parse()?;
            }
            "--rgb" => rgb = true,
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    Ok(Args {
        iface,
        auto,
        multicast,
        port,
        save,
        rgb,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let iface = match args.iface.clone() {
        Some(iface) => iface,
        None => {
            println!("Please specify the capture interface using --iface <name>.");
            print_usage();
            return Ok(());
        }
    };

    println!("GigE Vision capture (using FrameStream)");
    println!("  interface: {} (index {})", iface.name(), iface.index());
    if let Some(ip) = iface.ipv4() {
        println!("  interface IPv4: {ip}");
    }

    // Discover and connect to camera.
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

    // Connect to camera (fetches XML, builds nodemap).
    let mut camera = connect_gige(&device).await?;

    // Configure stream.
    let mut stream_device = viva_genicam::gige::GigeDevice::open(std::net::SocketAddr::new(
        std::net::IpAddr::V4(device.ip),
        viva_genicam::gige::GVCP_PORT,
    ))
    .await?;
    let mut builder = StreamBuilder::new(&mut stream_device).iface(iface.clone());
    if let Some(group) = args.multicast {
        builder = builder.multicast(Some(group));
    }
    if let Some(port) = args.port {
        builder = builder.destination_port(port);
    }
    if !args.auto {
        builder = builder.auto_packet_size(false);
    }
    let stream = builder.build().await?;

    // Create high-level frame stream (handles packet reassembly automatically).
    let time_sync = camera.time_sync().clone();
    let mut frame_stream = FrameStream::new(stream, Some(time_sync));

    // Start acquisition.
    camera.acquisition_start()?;

    let stats = frame_stream.stats_handle();
    let mut last_overlay = Instant::now();
    let mut frame_index = 0usize;
    let mut save_remaining = args.save;

    // Main acquisition loop - dramatically simplified!
    while let Some(frame) = frame_stream.next_frame().await? {
        frame_index += 1;
        print_frame_info(frame_index, &frame);

        if save_remaining > 0 {
            match save_frame(&frame, frame_index, args.rgb) {
                Ok(path) => println!("  saved {}", path.display()),
                Err(err) => warn!(error = %err, "failed to save frame"),
            }
            save_remaining = save_remaining.saturating_sub(1);
        }

        // Print stats overlay every second.
        if last_overlay.elapsed() >= Duration::from_secs(1) {
            print_overlay(&stats.snapshot());
            last_overlay = Instant::now();
        }

        // Stop after saving requested number of frames.
        if frame_index >= args.save {
            break;
        }
    }

    camera.acquisition_stop()?;
    println!("Capture stopped after {} frames.", frame_index);
    Ok(())
}

fn save_frame(frame: &Frame, index: usize, force_rgb: bool) -> Result<PathBuf, Box<dyn Error>> {
    let width =
        usize::try_from(frame.width).map_err(|_| "frame width exceeds host address space")?;
    let height =
        usize::try_from(frame.height).map_err(|_| "frame height exceeds host address space")?;
    let stem = format!("frame_{index:03}");

    if force_rgb || frame.pixel_format != PixelFormat::Mono8 {
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
