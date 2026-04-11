//! USB3 Vision CLI commands.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde::Serialize;
use tracing::info;

use viva_genicam::{Camera, U3vRegisterIo, connect_u3v};
use viva_pfnc::PixelFormat;
use viva_u3v::discovery::{U3vDeviceInfo, discover};
use viva_u3v::usb::RusbTransfer;

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
// get-usb / set-usb helpers
// ---------------------------------------------------------------------------

fn select_usb_device(index: Option<usize>) -> Result<U3vDeviceInfo> {
    let devices = discover().map_err(|e| anyhow!("USB discovery failed: {e}"))?;

    let idx = index.unwrap_or(0);
    devices
        .into_iter()
        .nth(idx)
        .ok_or_else(|| anyhow!("no USB3 Vision device at index {idx}"))
}

fn open_usb_camera(index: Option<usize>) -> Result<Camera<U3vRegisterIo<RusbTransfer>>> {
    let dev = select_usb_device(index)?;
    info!(
        vid = format!("{:04x}", dev.vendor_id),
        pid = format!("{:04x}", dev.product_id),
        "connecting to USB3 Vision camera"
    );
    connect_u3v(&dev).map_err(|e| anyhow!("connect failed: {e}"))
}

// ---------------------------------------------------------------------------
// get-usb
// ---------------------------------------------------------------------------

pub fn run_get(index: Option<usize>, name: String, json: bool) -> Result<()> {
    let camera = open_usb_camera(index)?;
    let value = camera.get(&name)?;

    if json {
        #[derive(Serialize)]
        struct GetResult {
            name: String,
            value: String,
        }
        common::print_json(&GetResult { name, value })?;
    } else {
        println!("{value}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// set-usb
// ---------------------------------------------------------------------------

pub fn run_set(index: Option<usize>, name: String, value: String, json: bool) -> Result<()> {
    let mut camera = open_usb_camera(index)?;
    camera.set(&name, &value)?;

    if json {
        #[derive(Serialize)]
        struct SetResult {
            name: String,
            value: String,
            ok: bool,
        }
        common::print_json(&SetResult {
            name,
            value,
            ok: true,
        })?;
    } else {
        println!("OK");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// stream-usb
// ---------------------------------------------------------------------------

pub fn run_stream(index: Option<usize>, save: usize, rgb: bool, duration_s: u64) -> Result<()> {
    let mut camera = open_usb_camera(index)?;

    // Read image dimensions from camera features.
    let width: u32 = camera.get("Width")?.parse().unwrap_or(640);
    let height: u32 = camera.get("Height")?.parse().unwrap_or(480);
    let pf_str = camera
        .get("PixelFormat")
        .unwrap_or_else(|_| "Mono8".to_string());
    let bpp: usize = match pf_str.as_str() {
        "RGB8Packed" | "RGB8" | "BGR8" | "BGR8Packed" => 3,
        "Mono16" => 2,
        _ => 1,
    };
    let payload_size = (width as usize) * (height as usize) * bpp;

    info!(
        width,
        height,
        pixel_format = pf_str,
        payload_size,
        "opening U3V stream"
    );

    // Open stream through the transport.
    let mut device = camera.transport().lock_device()?;
    let mut stream = device
        .open_stream(payload_size as u64)
        .map_err(|e| anyhow!("open stream failed: {e}"))?;

    // Start acquisition.
    drop(device); // Release lock before camera feature access.
    camera.set("AcquisitionStart", "1")?;

    let deadline = if duration_s > 0 {
        Some(std::time::Instant::now() + std::time::Duration::from_secs(duration_s))
    } else {
        None
    };

    let mut frame_count: u64 = 0;
    let mut saved: usize = 0;
    let t0 = std::time::Instant::now();

    loop {
        if let Some(deadline) = deadline {
            if std::time::Instant::now() >= deadline {
                break;
            }
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
            let payload = raw.payload.as_ref();
            let path = if rgb {
                let frame = viva_genicam::frame::Frame {
                    payload: raw.payload.clone(),
                    width: raw.leader.width,
                    height: raw.leader.height,
                    pixel_format,
                    chunks: None,
                    ts_dev: None,
                    ts_host: None,
                };
                let rgb_data = frame
                    .to_rgb8()
                    .map_err(|e| anyhow!("RGB conversion: {e}"))?;
                let path = PathBuf::from(format!("frame_{saved}.ppm"));
                let encoded = common::encode_ppm(raw.leader.width, raw.leader.height, &rgb_data)?;
                common::save_image(&encoded, &path)?;
                path
            } else {
                let path = PathBuf::from(format!("frame_{saved}.pgm"));
                let encoded = common::encode_pgm(raw.leader.width, raw.leader.height, payload)?;
                common::save_image(&encoded, &path)?;
                path
            };
            println!("  saved -> {}", path.display());
            saved += 1;
        }
    }

    println!(
        "\n{frame_count} frames in {:.2}s ({:.1} fps)",
        t0.elapsed().as_secs_f64(),
        frame_count as f64 / t0.elapsed().as_secs_f64().max(0.001),
    );

    Ok(())
}
