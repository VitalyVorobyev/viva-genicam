use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::common;

#[derive(Serialize)]
struct DeviceEntry {
    index: usize,
    ip: String,
    mac: String,
    manufacturer: Option<String>,
    model: Option<String>,
}

pub async fn run(timeout_ms: u64, iface: Option<Ipv4Addr>, json: bool) -> Result<()> {
    let timeout = Duration::from_millis(timeout_ms);
    let devices = common::discover_devices(timeout, iface).await?;
    info!(count = devices.len(), "discovered cameras");

    if json {
        let entries: Vec<DeviceEntry> = devices
            .iter()
            .enumerate()
            .map(|(idx, dev)| DeviceEntry {
                index: idx,
                ip: dev.ip.to_string(),
                mac: common::format_mac(&dev.mac),
                manufacturer: dev.manufacturer.clone(),
                model: dev.model.clone(),
            })
            .collect();
        common::print_json(&entries)?;
        return Ok(());
    }

    if devices.is_empty() {
        println!("No cameras discovered.");
        return Ok(());
    }

    println!(
        "{:<6} {:<16} {:<18} {:<20} Model",
        "INDEX", "IP", "MAC", "Manufacturer"
    );
    for (idx, dev) in devices.iter().enumerate() {
        println!(
            "{idx:<6} {:<16} {:<18} {:<20} {}",
            dev.ip,
            common::format_mac(&dev.mac),
            dev.manufacturer.as_deref().unwrap_or("-"),
            dev.model.as_deref().unwrap_or("-"),
        );
    }

    Ok(())
}
