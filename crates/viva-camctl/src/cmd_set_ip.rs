use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tracing::info;

use crate::common;

/// Parse a MAC address string like "DE:AD:BE:EF:CA:FE" into a 6-byte array.
fn parse_mac(s: &str) -> Result<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(anyhow!(
            "invalid MAC address '{s}': expected 6 colon-separated hex bytes"
        ));
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16)
            .with_context(|| format!("invalid hex byte '{part}' in MAC address"))?;
    }
    Ok(mac)
}

pub async fn run(
    mac: &str,
    ip: Ipv4Addr,
    subnet: Ipv4Addr,
    gateway: Ipv4Addr,
    force: bool,
    iface: Option<Ipv4Addr>,
) -> Result<()> {
    let mac = parse_mac(mac)?;

    if force {
        // FORCEIP: broadcast temporary IP assignment.
        let iface_obj = common::resolve_iface(iface)?;
        viva_gige::force_ip(mac, ip, subnet, gateway, iface_obj.as_ref())
            .await
            .context("FORCEIP command failed")?;
        println!(
            "FORCEIP sent: {} -> {} (subnet {}, gateway {})",
            common::format_mac(&mac),
            ip,
            subnet,
            gateway,
        );
    } else {
        // Persistent IP: discover device by MAC, then write registers.
        let timeout = Duration::from_millis(common::DEFAULT_DISCOVERY_TIMEOUT_MS);
        let devices = common::discover_devices(timeout, iface).await?;
        let device = devices.iter().find(|d| d.mac == mac).ok_or_else(|| {
            anyhow!(
                "no device with MAC {} found (use --force for offline assignment)",
                common::format_mac(&mac),
            )
        })?;

        info!(ip = %device.ip, "found device, opening control connection");
        let mut control = common::open_stream_device(device).await?;
        control.claim_control().await.context("claim CCP")?;

        control
            .write_persistent_ip(ip, subnet, gateway)
            .await
            .context("write persistent IP registers")?;
        control
            .enable_persistent_ip()
            .await
            .context("enable persistent IP mode")?;

        control.release_control().await.context("release CCP")?;
        println!(
            "Persistent IP configured: {} -> {} (subnet {}, gateway {})",
            common::format_mac(&mac),
            ip,
            subnet,
            gateway,
        );
        println!("Power-cycle the device to apply the new IP address.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mac_valid() {
        let mac = parse_mac("DE:AD:BE:EF:CA:FE").unwrap();
        assert_eq!(mac, [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
    }

    #[test]
    fn parse_mac_lowercase() {
        let mac = parse_mac("de:ad:be:ef:ca:fe").unwrap();
        assert_eq!(mac, [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
    }

    #[test]
    fn parse_mac_invalid_format() {
        assert!(parse_mac("DEADBEEFCAFE").is_err());
        assert!(parse_mac("DE:AD:BE:EF:CA").is_err());
        assert!(parse_mac("DE:AD:BE:EF:CA:FE:00").is_err());
    }

    #[test]
    fn parse_mac_invalid_hex() {
        assert!(parse_mac("GG:AD:BE:EF:CA:FE").is_err());
    }
}
