//! GigE Vision action commands (backlog ST-54): what `viva-camctl action`
//! sends, from Studio. An action is addressed by keys, not by camera: every
//! device the address reaches compares the keys and group mask with its own
//! `ActionDeviceKey`, `ActionGroupKey` and `ActionGroupMask`, and those that
//! match act and acknowledge. Embedded mode only.

use std::net::Ipv4Addr;

use tauri::State;
use viva_genicam::gige::ActionParams;

use crate::backend::BackendState;

/// The longest Studio waits for acknowledgements.
const MAX_ACTION_TIMEOUT_MS: u32 = 10_000;

/// A device that answered the action command with an error status.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ActionRefusal {
    /// Who answered.
    pub source: String,
    /// The status it answered with, such as `NOT_IMPLEMENTED`.
    pub status: String,
}

/// What sending an action command did.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ActionCommandResult {
    /// Where it was sent.
    pub destination: String,
    /// Datagrams sent.
    pub sent: usize,
    /// Devices that acknowledged it within the timeout.
    pub acks: usize,
    /// Devices that refused it, each once.
    pub refusals: Vec<ActionRefusal>,
}

/// A 32-bit key or mask, in hexadecimal, with or without `0x`.
fn parse_key(label: &str, text: &str) -> Result<u32, String> {
    let digits = text.trim();
    let digits = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .unwrap_or(digits);
    if digits.is_empty() || digits.len() > 8 {
        return Err(format!(
            "{label}: '{text}' is not a 32-bit hexadecimal value"
        ));
    }
    u32::from_str_radix(digits, 16)
        .map_err(|_| format!("{label}: '{text}' is not a 32-bit hexadecimal value"))
}

/// A time in device ticks: decimal, or hexadecimal after `0x`.
fn parse_ticks(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| format!("Scheduled time: '{text}' is not a 64-bit tick count"))
}

/// Where an action command goes by default: the broadcast address of the
/// network the connected camera was found on, or the limited broadcast
/// `255.255.255.255` without one. Refused in remote mode.
#[tauri::command]
pub async fn default_action_destination(
    backend: State<'_, BackendState>,
) -> Result<String, String> {
    Ok(backend.default_action_destination().await?.to_string())
}

/// Send an action command to `destination` — a broadcast address reaches
/// every device on that network — and collect acknowledgements for
/// `timeout_ms`. `scheduled_time`, in device ticks, makes it a scheduled
/// action.
#[tauri::command]
pub async fn send_action_command(
    destination: String,
    device_key: String,
    group_key: String,
    group_mask: String,
    scheduled_time: Option<String>,
    timeout_ms: u32,
    backend: State<'_, BackendState>,
) -> Result<ActionCommandResult, String> {
    let address: Ipv4Addr = destination
        .trim()
        .parse()
        .map_err(|_| format!("Destination: '{destination}' is not an IPv4 address"))?;
    let params = ActionParams {
        device_key: parse_key("Device key", &device_key)?,
        group_key: parse_key("Group key", &group_key)?,
        group_mask: parse_key("Group mask", &group_mask)?,
        scheduled_time: match scheduled_time.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => Some(parse_ticks(text)?),
        },
    };
    let timeout_ms = timeout_ms.min(MAX_ACTION_TIMEOUT_MS);
    let summary = backend
        .send_action(address, params, u64::from(timeout_ms))
        .await?;
    Ok(ActionCommandResult {
        destination: address.to_string(),
        sent: summary.sent,
        acks: summary.acks,
        refusals: summary
            .refusals
            .iter()
            .map(|refusal| ActionRefusal {
                source: refusal.source.to_string(),
                status: refusal.status.to_string(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_key, parse_ticks};

    #[test]
    fn keys_are_hexadecimal_with_or_without_a_prefix() {
        assert_eq!(parse_key("k", "0x42").unwrap(), 0x42);
        assert_eq!(parse_key("k", "FFFFFFFF").unwrap(), u32::MAX);
        assert_eq!(parse_key("k", " 0Xab ").unwrap(), 0xAB);
        for bad in ["", "0x", "123456789", "xyz", "-1"] {
            assert!(
                parse_key("Group key", bad)
                    .unwrap_err()
                    .contains("Group key"),
                "{bad}"
            );
        }
    }

    #[test]
    fn ticks_are_decimal_or_prefixed_hexadecimal() {
        assert_eq!(parse_ticks("1000").unwrap(), 1000);
        assert_eq!(parse_ticks("0x10").unwrap(), 16);
        assert_eq!(parse_ticks("18446744073709551615").unwrap(), u64::MAX);
        assert!(parse_ticks("ten").is_err());
    }
}
