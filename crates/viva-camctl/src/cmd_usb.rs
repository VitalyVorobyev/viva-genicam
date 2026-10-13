//! USB3 Vision CLI commands: `list-usb`, and what `stream`, `bench`,
//! `events` and `chunks` do with `--usb`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde::Serialize;
use tracing::{info, warn};

use viva_genicam::{
    Camera, ChunkConfig, GenicamError, StreamStats, U3vFrameStream, U3vRegisterIo,
    U3vStreamOptions, connect_u3v,
};
use viva_u3v::{NusbTransfer, UsbTransfer};
use viva_u3v::{U3vDeviceInfo, discover};

use crate::common;

// ---------------------------------------------------------------------------
// list-usb
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct UsbDeviceEntry {
    index: usize,
    vendor_id: String,
    product_id: String,
    manufacturer: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    bus: String,
    address: u8,
    speed: Option<String>,
}

pub fn run_list(json: bool) -> Result<()> {
    let devices = discover().map_err(|e| anyhow!("USB discovery failed: {e}"))?;
    info!(count = devices.len(), "discovered USB3 Vision cameras");

    if json {
        let entries: Vec<UsbDeviceEntry> = devices
            .iter()
            .enumerate()
            .map(|(idx, dev)| UsbDeviceEntry {
                index: idx,
                vendor_id: format!("{:04x}", dev.vendor_id),
                product_id: format!("{:04x}", dev.product_id),
                manufacturer: dev.manufacturer.clone(),
                model: dev.model.clone(),
                serial: dev.serial.clone(),
                bus: dev.bus.clone(),
                address: dev.address,
                speed: dev.speed.map(|s| s.to_string()),
            })
            .collect();
        common::print_json(&entries)?;
        return Ok(());
    }

    if devices.is_empty() {
        println!("No USB3 Vision cameras discovered.");
        return Ok(());
    }

    println!(
        "{:<6} {:<10} {:<20} {:<20} {:<12} Speed",
        "INDEX", "VID:PID", "Manufacturer", "Model", "Serial"
    );
    for (idx, dev) in devices.iter().enumerate() {
        println!(
            "{idx:<6} {:04x}:{:04x}  {:<20} {:<20} {:<12} {}",
            dev.vendor_id,
            dev.product_id,
            dev.manufacturer.as_deref().unwrap_or("-"),
            dev.model.as_deref().unwrap_or("-"),
            dev.serial.as_deref().unwrap_or("-"),
            dev.speed.map_or("-".to_string(), |s| s.to_string()),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Opening a camera
// ---------------------------------------------------------------------------

pub(crate) fn select_usb_device(index: Option<usize>) -> Result<U3vDeviceInfo> {
    let devices = discover().map_err(|e| anyhow!("USB discovery failed: {e}"))?;

    let idx = index.unwrap_or(0);
    devices
        .into_iter()
        .nth(idx)
        .ok_or_else(|| anyhow!("no USB3 Vision device at index {idx}"))
}

pub(crate) fn open_usb_camera(index: Option<usize>) -> Result<Camera<U3vRegisterIo<NusbTransfer>>> {
    let dev = select_usb_device(index)?;
    info!(
        vid = format!("{:04x}", dev.vendor_id),
        pid = format!("{:04x}", dev.product_id),
        "connecting to USB3 Vision camera"
    );
    connect_u3v(&dev).map_err(|e| anyhow!("connect failed: {e}"))
}

/// The next complete frame within `timeout`, `None` at the end of the
/// stream; an error ends the stream.
fn next_frame(
    frames: &mut U3vFrameStream,
    timeout: Duration,
) -> Result<Option<viva_genicam::Frame>> {
    match frames.next_frame_blocking(timeout) {
        Ok(frame) => Ok(frame),
        Err(GenicamError::Timeout(_)) => Err(anyhow!("no frame within {timeout:?}")),
        Err(err) => Err(err.into()),
    }
}

// ---------------------------------------------------------------------------
// stream --usb
// ---------------------------------------------------------------------------

/// `stream --usb`: stream frames, save the first `save`, print each frame's
/// chunks when it carries some, and the counters at the end.
pub fn run_stream(
    index: usize,
    save: usize,
    save_format: common::SaveFormat,
    rgb: bool,
    duration_s: u64,
) -> Result<()> {
    let mut camera = open_usb_camera(Some(index))?;
    let mut frames = camera.open_stream(U3vStreamOptions::default())?;
    // Sets TLParamsLocked too.
    camera.acquisition_start()?;

    let deadline = (duration_s > 0).then(|| Instant::now() + Duration::from_secs(duration_s));
    let mut frame_count: u64 = 0;
    let mut saved: usize = 0;
    let t0 = Instant::now();
    let mut ended = None;

    while deadline.is_none_or(|deadline| Instant::now() < deadline) {
        let frame = match next_frame(&mut frames, Duration::from_secs(5)) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(err) => {
                ended = Some(err);
                break;
            }
        };
        frame_count += 1;
        let fps = frame_count as f64 / t0.elapsed().as_secs_f64().max(0.001);
        println!(
            "frame #{frame_count}: {}x{} {} block={} ts={} ({fps:.1} fps)",
            frame.width,
            frame.height,
            frame.pixel_format,
            frame.block_id,
            frame.ts_dev.unwrap_or(0),
        );
        if frame.chunks.is_some() {
            for (name, value) in camera.chunk_features(&frame) {
                match value {
                    Ok(value) => println!("  {name} = {value}"),
                    Err(err) => println!("  {name}: {err}"),
                }
            }
        }

        if saved < save {
            let (encoded, ext) = common::encode_frame(
                frame.pixel_format,
                frame.width,
                frame.height,
                frame.row_stride(),
                frame.payload.as_ref(),
                save_format,
                rgb,
            )?;
            let path = PathBuf::from(format!("frame_{saved}.{ext}"));
            common::save_image(&encoded, &path)?;
            println!("  saved -> {}", path.display());
            saved += 1;
        }
    }

    // Clears TLParamsLocked as well.
    if let Err(err) = camera.acquisition_stop() {
        eprintln!("AcquisitionStop failed: {err}");
    }
    let stats = frames.stats();
    frames.close();
    print_counters("Summary:", &stats);
    if let Some(err) = ended {
        eprintln!("stream error: {err:#}");
        if camera.transport().is_lost() {
            eprintln!("the camera is gone: unplugged or reset");
        }
        return Err(err);
    }
    Ok(())
}

/// [`common::print_stream_counters`] and what only a USB3 Vision stream
/// counts: its transfers and the failed transfers it recovered from.
fn print_counters(prefix: &str, stats: &StreamStats) {
    common::print_stream_counters(prefix, stats);
    println!(
        "{} transfers={} skipped={} transfer_errors={} MB/s={:.1}",
        " ".repeat(prefix.len()),
        stats.packets_received,
        stats.packets_ignored_malformed,
        stats.transfer_errors,
        stats.bytes as f64 / 1e6 / stats.elapsed.as_secs_f64().max(f64::EPSILON),
    );
}

// ---------------------------------------------------------------------------
// bench --usb
// ---------------------------------------------------------------------------

/// A USB3 Vision streaming benchmark's result: the stream's own counters.
#[derive(Debug, Serialize)]
pub struct UsbBenchReport {
    /// How long the benchmark streamed.
    pub duration_s: f64,
    /// Frames delivered complete.
    pub frames: u64,
    /// Blocks given up or ended with an error status.
    pub frames_incomplete: u64,
    /// Block ids skipped.
    pub frames_missed: u64,
    /// Frames dropped because they were not taken in time.
    pub frames_backpressure: u64,
    /// Payload bytes of complete frames.
    pub bytes: u64,
    /// Bulk transfers read.
    pub transfers: u64,
    /// Transfers that were not the leader, payload or trailer due.
    pub transfers_skipped: u64,
    /// Failed transfers the stream recovered from.
    pub transfer_errors: u64,
    pub avg_fps: f64,
    pub avg_mbps: f64,
}

/// Stream from `camera` for `duration`, counting what arrives. Acquisition
/// is started and stopped, and the stream closed, here.
pub fn bench<T: UsbTransfer + 'static>(
    camera: &mut Camera<U3vRegisterIo<T>>,
    duration: Duration,
) -> Result<UsbBenchReport> {
    let mut frames = camera.open_stream(U3vStreamOptions::default())?;
    camera.acquisition_start()?;
    let started = Instant::now();
    let mut last_print = started;
    let mut failure = None;
    while started.elapsed() < duration {
        match frames.next_frame_blocking(Duration::from_millis(200)) {
            Ok(Some(_)) | Err(GenicamError::Timeout(_)) => {}
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, "the stream failed");
                failure = Some(err);
                break;
            }
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            last_print = Instant::now();
            print_counters("[bench]", &frames.stats());
        }
    }
    let elapsed = started.elapsed().as_secs_f64().max(f64::EPSILON);
    if let Err(err) = camera.acquisition_stop() {
        eprintln!("AcquisitionStop failed: {err}");
    }
    let stats = frames.stats();
    frames.close();
    if let Some(err) = failure
        && stats.frames_complete == 0
    {
        return Err(err.into());
    }
    Ok(UsbBenchReport {
        duration_s: elapsed,
        frames: stats.frames_complete,
        frames_incomplete: stats.frames_incomplete,
        frames_missed: stats.frames_missed,
        frames_backpressure: stats.frames_backpressure,
        bytes: stats.bytes,
        transfers: stats.packets_received,
        transfers_skipped: stats.packets_ignored_malformed,
        transfer_errors: stats.transfer_errors,
        avg_fps: stats.frames_complete as f64 / elapsed,
        avg_mbps: stats.bytes as f64 * 8.0 / 1e6 / elapsed,
    })
}

pub fn run_bench(
    index: usize,
    duration_s: u64,
    json: bool,
    json_out: Option<&std::path::Path>,
) -> Result<()> {
    let mut camera = open_usb_camera(Some(index))?;
    let report = bench(&mut camera, Duration::from_secs(duration_s.max(1)))?;
    println!(
        "Summary: {} frames complete, {} incomplete, {} missed, {} backpressure; \
         {:.1} fps, {:.1} Mbit/s ({:.1} MB/s); {} transfers, {} skipped, {} transfer errors",
        report.frames,
        report.frames_incomplete,
        report.frames_missed,
        report.frames_backpressure,
        report.avg_fps,
        report.avg_mbps,
        report.avg_mbps / 8.0,
        report.transfers,
        report.transfers_skipped,
        report.transfer_errors,
    );
    if let Some(path) = json_out {
        let file =
            std::fs::File::create(path).map_err(|e| anyhow!("create {}: {e}", path.display()))?;
        serde_json::to_writer_pretty(file, &report)
            .map_err(|e| anyhow!("write {}: {e}", path.display()))?;
    } else if json {
        common::print_json(&report)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// events --usb
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct UsbEventRecord {
    index: usize,
    id: u16,
    ts_dev: u64,
    features: Vec<(String, String)>,
}

/// `events --usb`: turn on the events named, stream while listening — a
/// USB3 Vision camera admits one process, so nothing else can start its
/// acquisition — and print `count` events with the features each feeds.
pub fn run_events(index: usize, enable: &str, count: u32, json: bool) -> Result<()> {
    let mut camera = open_usb_camera(Some(index))?;
    let names: Vec<&str> = enable
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    camera.configure_events(&names)?;
    let events = camera.open_event_stream()?;
    let frames = camera.open_stream(U3vStreamOptions::default())?;
    camera.acquisition_start()?;

    let mut records = Vec::new();
    for idx in 0..usize::try_from(count).unwrap_or(0) {
        let event = match events.next_blocking(Duration::from_secs(5)) {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, "failed to receive event");
                break;
            }
        };
        let features: Vec<(String, String)> = camera
            .event_features(&event)
            .into_iter()
            .map(|(name, value)| (name, value.unwrap_or_else(|e| format!("<{e}>"))))
            .collect();
        if json {
            records.push(UsbEventRecord {
                index: idx + 1,
                id: event.id,
                ts_dev: event.ts_dev,
                features,
            });
        } else {
            println!(
                "#{:02} id=0x{:04X} ticks={} payload={} bytes",
                idx + 1,
                event.id,
                event.ts_dev,
                event.data.len()
            );
            for (name, value) in features {
                println!("    {name} = {value}");
            }
        }
    }
    if let Err(err) = camera.acquisition_stop() {
        eprintln!("AcquisitionStop failed: {err}");
    }
    frames.close();
    drop(events);
    if json {
        common::print_json(&records)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// chunks --usb
// ---------------------------------------------------------------------------

/// `chunks --usb`: turn chunk mode and the selectors named on or off.
pub fn run_chunks(index: usize, enable: bool, selectors: &str) -> Result<()> {
    let mut camera = open_usb_camera(Some(index))?;
    let selectors: Vec<String> = selectors
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    camera.configure_chunks(&ChunkConfig {
        selectors: selectors.clone(),
        active: enable,
    })?;
    println!(
        "Chunk mode {} ({})",
        if enable { "enabled" } else { "disabled" },
        if selectors.is_empty() {
            "no selectors".to_string()
        } else {
            selectors.join(", ")
        }
    );
    Ok(())
}
