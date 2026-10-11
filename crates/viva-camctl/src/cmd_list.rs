use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_gige::nic::IfaceSelector;
use viva_gige::{DeviceInfo, DiscoveryOptions};

use crate::common;

/// What `list` was asked for.
pub struct ListArgs {
    pub timeout_ms: u64,
    pub iface: Option<IfaceSelector>,
    /// Ask this address directly instead of broadcasting.
    pub ip: Option<Ipv4Addr>,
    pub allow_broadcast_ack: bool,
}

#[derive(Serialize)]
struct DeviceEntry {
    index: usize,
    ip: String,
    mac: String,
    manufacturer: Option<String>,
    model: Option<String>,
    subnet: Option<String>,
    gateway: Option<String>,
    /// The host address that heard the camera.
    host: Option<String>,
    /// `false` for a camera configured for a subnet the host is not on.
    on_host_subnet: Option<bool>,
}

pub async fn run(args: ListArgs, json: bool) -> Result<()> {
    let timeout = Duration::from_millis(args.timeout_ms);
    let devices = match args.ip {
        Some(ip) => viva_gige::discover_ip(ip, timeout)
            .await
            .context("unicast discovery")?
            .into_iter()
            .collect(),
        None => {
            let mut options = DiscoveryOptions::new().allow_broadcast_ack(args.allow_broadcast_ack);
            if let Some(selector) = &args.iface {
                options = options.interface(common::resolve_iface_selector(selector)?.name());
            }
            viva_gige::discover_with(timeout, &options)
                .await
                .context("broadcast discovery")?
        }
    };
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
                subnet: dev.subnet.map(|ip| ip.to_string()),
                gateway: dev.gateway.map(|ip| ip.to_string()),
                host: dev.local_ip.map(|ip| ip.to_string()),
                on_host_subnet: dev.shares_subnet_with_host(),
            })
            .collect();
        common::print_json(&entries)?;
        return Ok(());
    }

    if devices.is_empty() {
        match args.ip {
            Some(ip) => println!("No camera answered at {ip}."),
            None => println!("No cameras discovered."),
        }
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
    for (idx, dev) in devices.iter().enumerate() {
        if let Some(note) = subnet_warning(dev) {
            println!("\nWARNING [{idx}]: {note}");
        }
    }

    Ok(())
}

/// Why a camera that answered cannot be reached, if it cannot.
fn subnet_warning(dev: &DeviceInfo) -> Option<String> {
    if dev.shares_subnet_with_host() != Some(false) {
        return None;
    }
    let host = dev.local_ip?;
    let host_bits = dev.local_netmask.map_or(0, |m| u32::from(m).leading_ones());
    let camera_bits = dev.subnet.map_or(0, |m| u32::from(m).leading_ones());
    Some(format!(
        "{} is configured for {}/{camera_bits}, which the host address {host}/{host_bits} \
         is not on. It answered a broadcast, but connecting to it will fail until it \
         gets an address on the host's subnet (`viva-camctl set-ip`) or the host joins \
         its network.",
        dev.ip, dev.ip
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_on_another_subnet_gets_a_warning_and_one_on_ours_none() {
        let mut dev = DeviceInfo::from_ip(Ipv4Addr::new(192, 168, 77, 10));
        dev.subnet = Some(Ipv4Addr::new(255, 255, 255, 0));
        dev.local_ip = Some(Ipv4Addr::new(169, 254, 1, 2));
        dev.local_netmask = Some(Ipv4Addr::new(255, 255, 0, 0));
        let note = subnet_warning(&dev).expect("a warning");
        assert!(note.contains("192.168.77.10/24"), "{note}");
        assert!(note.contains("169.254.1.2/16"), "{note}");

        dev.local_ip = Some(Ipv4Addr::new(192, 168, 77, 2));
        dev.local_netmask = Some(Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(subnet_warning(&dev), None);
    }
}
