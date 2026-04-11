//! USB3 Vision CLI commands.

use anyhow::{Result, anyhow};
use serde::Serialize;
use tracing::info;

use viva_genicam::{Camera, U3vRegisterIo, connect_u3v};
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
