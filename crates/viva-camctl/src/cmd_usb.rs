//! USB3 Vision CLI commands.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde::Serialize;
use tracing::{info, warn};

use viva_genicam::{Camera, U3vRegisterIo, connect_u3v};
use viva_pfnc::PixelFormat;
use viva_u3v::U3vStream;
use viva_u3v::{RusbTransfer, UsbTransfer};
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
    bus: u8,
    address: u8,
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
                bus: dev.bus,
                address: dev.address,
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
        "{:<6} {:<10} {:<20} {:<20} Serial",
        "INDEX", "VID:PID", "Manufacturer", "Model"
    );
    for (idx, dev) in devices.iter().enumerate() {
        println!(
            "{idx:<6} {:04x}:{:04x}  {:<20} {:<20} {}",
            dev.vendor_id,
            dev.product_id,
            dev.manufacturer.as_deref().unwrap_or("-"),
            dev.model.as_deref().unwrap_or("-"),
            dev.serial.as_deref().unwrap_or("-"),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Opening a camera
// ---------------------------------------------------------------------------

fn select_usb_device(index: Option<usize>) -> Result<U3vDeviceInfo> {
    let devices = discover().map_err(|e| anyhow!("USB discovery failed: {e}"))?;

    let idx = index.unwrap_or(0);
    devices
        .into_iter()
        .nth(idx)
        .ok_or_else(|| anyhow!("no USB3 Vision device at index {idx}"))
}

pub(crate) fn open_usb_camera(index: Option<usize>) -> Result<Camera<U3vRegisterIo<RusbTransfer>>> {
    let dev = select_usb_device(index)?;
    info!(
        vid = format!("{:04x}", dev.vendor_id),
        pid = format!("{:04x}", dev.product_id),
        "connecting to USB3 Vision camera"
    );
    connect_u3v(&dev).map_err(|e| anyhow!("connect failed: {e}"))
}

// ---------------------------------------------------------------------------
// stream-usb
// ---------------------------------------------------------------------------

/// Size the payload from the camera's image format and open its stream.
fn open_raw_stream<T: UsbTransfer + 'static>(
    camera: &mut Camera<U3vRegisterIo<T>>,
) -> Result<U3vStream<T>> {
    let width: u32 = camera.get("Width")?.parse().unwrap_or(640);
    let height: u32 = camera.get("Height")?.parse().unwrap_or(480);
    let pf_str = camera
        .get("PixelFormat")
        .unwrap_or_else(|_| "Mono8".to_string());
    // Sized from the PFNC code, so a packed format is not undersized.
    let payload_size = PixelFormat::from_name(&pf_str)
        .image_bytes(width, height)
        .unwrap_or(width as usize * height as usize);
    info!(
        width,
        height,
        pixel_format = pf_str,
        payload_size,
        "opening U3V stream"
    );
    let mut device = camera.transport().lock_device()?;
    device
        .open_stream(payload_size as u64)
        .map_err(|e| anyhow!("open stream failed: {e}"))
}

pub fn run_stream(
    index: Option<usize>,
    save: usize,
    save_format: common::SaveFormat,
    rgb: bool,
    duration_s: u64,
) -> Result<()> {
    let mut camera = open_usb_camera(index)?;

    let mut stream = open_raw_stream(&mut camera)?;

    // Sets TLParamsLocked too.
    camera.acquisition_start()?;

    let deadline = if duration_s > 0 {
        Some(std::time::Instant::now() + std::time::Duration::from_secs(duration_s))
    } else {
        None
    };

    let mut frame_count: u64 = 0;
    let mut saved: usize = 0;
    let t0 = std::time::Instant::now();

    loop {
        if let Some(deadline) = deadline
            && std::time::Instant::now() >= deadline
        {
            break;
        }

        let raw = match stream.next_frame() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("stream error: {e}");
                break;
            }
        };

        frame_count += 1;
        let elapsed = t0.elapsed().as_secs_f64();
        let fps = frame_count as f64 / elapsed.max(0.001);

        let pixel_format = PixelFormat::from_code(raw.leader.pixel_format);
        println!(
            "frame #{frame_count}: {}x{} {pixel_format} block={} ({fps:.1} fps)",
            raw.leader.width, raw.leader.height, raw.trailer.block_id,
        );

        // Save frames if requested.
        if saved < save {
            let (encoded, ext) = common::encode_frame(
                pixel_format,
                raw.leader.width,
                raw.leader.height,
                (raw.leader.x_padding != 0)
                    .then(|| pixel_format.line_bytes(raw.leader.width))
                    .flatten()
                    .map(|line| line + usize::from(raw.leader.x_padding)),
                raw.payload.as_ref(),
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
    println!(
        "\n{frame_count} frames in {:.2}s ({:.1} fps)",
        t0.elapsed().as_secs_f64(),
        frame_count as f64 / t0.elapsed().as_secs_f64().max(0.001),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// bench --usb
// ---------------------------------------------------------------------------

/// A USB3 Vision streaming benchmark's result. Fewer counters than GigE
/// Vision's: the USB3 Vision stream has no receive engine counting packets
/// and resends (backlog API-11), so this counts blocks as they arrive.
#[derive(Debug, Serialize)]
pub struct UsbBenchReport {
    /// How long the benchmark streamed.
    pub duration_s: f64,
    /// Blocks received.
    pub frames: u64,
    /// Of those, blocks whose trailer reported a status other than success.
    pub frames_failed: u64,
    /// Payload bytes received.
    pub bytes: u64,
    /// Read errors from the stream endpoint.
    pub stream_errors: u64,
    pub avg_fps: f64,
    pub avg_mbps: f64,
}

/// Stream from `camera` for `duration`, counting what arrives. Acquisition
/// is started and stopped, and the stream closed, here.
pub fn bench<T: UsbTransfer + 'static>(
    camera: &mut Camera<U3vRegisterIo<T>>,
    duration: Duration,
) -> Result<UsbBenchReport> {
    let mut stream = open_raw_stream(camera)?;
    camera.acquisition_start()?;
    let started = Instant::now();
    let (mut frames, mut failed, mut bytes, mut errors) = (0u64, 0u64, 0u64, 0u64);
    let mut last_print = started;
    while started.elapsed() < duration {
        match stream.next_frame_within(Duration::from_millis(200)) {
            Ok(Some(frame)) => {
                frames += 1;
                bytes += frame.payload.len() as u64;
                failed += u64::from(frame.trailer.status != 0);
            }
            Ok(None) => {}
            Err(err) => {
                errors += 1;
                warn!(error = %err, "stream read failed");
                // A device that is gone fails every read at once.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            last_print = Instant::now();
            println!("[bench] {frames} frames, {failed} failed, {errors} read errors");
        }
    }
    let elapsed = started.elapsed().as_secs_f64().max(f64::EPSILON);
    if let Err(err) = camera.acquisition_stop() {
        eprintln!("AcquisitionStop failed: {err}");
    }
    drop(stream);
    if let Err(err) = camera.transport().lock_device()?.stop_stream() {
        eprintln!("stopping the stream failed: {err}");
    }
    Ok(UsbBenchReport {
        duration_s: elapsed,
        frames,
        frames_failed: failed,
        bytes,
        stream_errors: errors,
        avg_fps: frames as f64 / elapsed,
        avg_mbps: bytes as f64 * 8.0 / 1e6 / elapsed,
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
        "Summary: {} frames ({} failed), {:.1} fps, {:.1} Mbit/s, {} read errors",
        report.frames, report.frames_failed, report.avg_fps, report.avg_mbps, report.stream_errors
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
