use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::sync::Mutex;
use tracing::{info, warn};
use viva_genapi_xml::XmlError;
use viva_genicam::StreamStats;
use viva_genicam::genapi::NodeMap;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::nic::IfaceSelector;
use viva_genicam::pfnc::PixelFormat;
use viva_genicam::{Camera, Frame, FrameStream, GigeRegisterIo, StreamBuilder, StreamDest};

#[derive(Debug, Clone)]
struct Args {
    iface: IfaceSelector,
    group: Ipv4Addr,
    port: u16,
    ttl: u32,
    loopback: bool,
    stream_idx: u32,
    packet_size: Option<u32>,
    save: usize,
}

fn print_usage() {
    eprintln!(
        "usage: multicast_grab --iface <HOST-IP|NAME> --group <IPv4> --port <n> [--ttl <n>] [--loopback] [--stream-idx <n>] [--packet-size <bytes>] [--save <n>]"
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
    let mut packet_size: Option<u32> = None;
    let mut save = 0usize;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--iface requires a host IP or interface name".to_string())?;
                // Either spelling, as everywhere else (#109).
                iface = Some(value.parse::<IfaceSelector>().unwrap());
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
        packet_size,
        save,
    })
}

#[tokio::main]
#[allow(clippy::await_holding_lock)] // the stream is configured under the device lock
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let iface = args.iface.resolve()?;

    println!("GigE Vision multicast capture");
    println!("  interface: {} (index {})", iface.name(), iface.index());
    println!("  interface IPv4: {:?}", iface.ipv4());
    println!("  multicast group: {}", args.group);
    println!("  port: {}", args.port);
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
                    .map_err(|err| XmlError::Transport(err.to_string()))
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

    camera.configure_stream_multicast(args.stream_idx, args.group, args.port)?;

    // The stream channel is written over the camera's own control connection:
    // only the application holding control may configure it.
    let mut stream_device = camera.transport().lock_device()?;
    let mut builder = StreamBuilder::new(&mut stream_device)
        .iface(iface.clone())
        .dest(StreamDest::Multicast {
            group: args.group,
            port: args.port,
            loopback: args.loopback,
            ttl: args.ttl,
        })
        .channel(args.stream_idx);
    if let Some(size) = args.packet_size {
        builder = builder.packet_size(size);
    } else {
        builder = builder.auto_packet_size();
    }
    let stream = builder.build().await?;
    drop(stream_device);

    // The receive engine assembles the blocks and keeps the counters.
    let mut frames = FrameStream::new(stream, Some(camera.time_sync().clone()));
    camera.acquisition_start()?;
    let mut last_overlay = Instant::now();
    let mut last_pixel_format: Option<PixelFormat> = None;
    let mut frame_index = 0usize;
    let mut save_remaining = args.save;

    println!("Joined multicast group; waiting for frames...");

    loop {
        let frame = match frames.next_frame().await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, "stream failed; stopping capture");
                break;
            }
        };
        if last_pixel_format != Some(frame.pixel_format) {
            info!(
                block_id = frame.block_id,
                width = frame.width,
                height = frame.height,
                pixel_format = %frame.pixel_format,
                "detected pixel format"
            );
            last_pixel_format = Some(frame.pixel_format);
        }
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
            print_overlay(&frames.stats());
            last_overlay = Instant::now();
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
        "[stats] fps={:.1} Mbps={:.2} lost={} latency={}",
        stats.avg_fps,
        stats.avg_mbps,
        stats.frames_lost(),
        latency
    );
}
