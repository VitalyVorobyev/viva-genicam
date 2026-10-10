//! IP configuration commands for GigE Vision cameras.
//!
//! Provides ForceIP (temporary) and persistent IP configuration
//! via direct GVCP register access. Requires embedded backend mode.

use std::net::Ipv4Addr;

use tauri::State;

use crate::backend::{BackendState, NetworkConfig};

/// Force a temporary IP address on a GigE camera identified by MAC address.
///
/// This is a broadcast command — the target device does not need to be connected.
/// The assigned IP is temporary and does not survive a power cycle.
#[tauri::command]
pub async fn force_ip(
    mac: String,
    ip: String,
    subnet: String,
    gateway: String,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let mac_bytes = parse_mac(&mac)?;
    let ip_addr: Ipv4Addr = ip.parse().map_err(|e| format!("Invalid IP: {e}"))?;
    let subnet_addr: Ipv4Addr = subnet.parse().map_err(|e| format!("Invalid subnet: {e}"))?;
    let gateway_addr: Ipv4Addr = gateway
        .parse()
        .map_err(|e| format!("Invalid gateway: {e}"))?;
    backend
        .force_ip(mac_bytes, ip_addr, subnet_addr, gateway_addr)
        .await
}

/// Read the network configuration of the connected GigE camera.
#[tauri::command]
pub async fn get_network_config(backend: State<'_, BackendState>) -> Result<NetworkConfig, String> {
    backend.get_network_config().await
}

/// Set persistent IP configuration on a connected GigE camera.
///
/// Writes IP/subnet/gateway to non-volatile registers and enables
/// persistent IP mode. The configuration survives power cycles.
#[tauri::command]
pub async fn set_persistent_ip(
    ip: String,
    subnet: String,
    gateway: String,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let ip_addr: Ipv4Addr = ip.parse().map_err(|e| format!("Invalid IP: {e}"))?;
    let subnet_addr: Ipv4Addr = subnet.parse().map_err(|e| format!("Invalid subnet: {e}"))?;
    let gateway_addr: Ipv4Addr = gateway
        .parse()
        .map_err(|e| format!("Invalid gateway: {e}"))?;

    backend
        .set_persistent_ip(ip_addr, subnet_addr, gateway_addr)
        .await
}

/// Parse a MAC address string (XX:XX:XX:XX:XX:XX or XX-XX-XX-XX-XX-XX) to bytes.
fn parse_mac(mac: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = mac.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(format!(
            "Invalid MAC address '{mac}': expected 6 colon-separated hex bytes"
        ));
    }
    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] =
            u8::from_str_radix(part, 16).map_err(|e| format!("Invalid MAC byte '{part}': {e}"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::parse_mac;

    #[test]
    fn test_parse_mac_colon_separated() {
        let result = parse_mac("00:1A:2B:3C:4D:5E").unwrap();
        assert_eq!(result, [0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E]);
    }

    #[test]
    fn test_parse_mac_dash_separated() {
        let result = parse_mac("AA-BB-CC-DD-EE-FF").unwrap();
        assert_eq!(result, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_parse_mac_lowercase() {
        let result = parse_mac("ab:cd:ef:01:23:45").unwrap();
        assert_eq!(result, [0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45]);
    }

    #[test]
    fn test_parse_mac_invalid_length() {
        assert!(parse_mac("00:11:22:33:44").is_err());
    }

    #[test]
    fn test_parse_mac_invalid_hex() {
        assert!(parse_mac("GG:HH:II:JJ:KK:LL").is_err());
    }
}
