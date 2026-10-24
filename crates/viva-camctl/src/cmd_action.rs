//! `action`: send a GigE Vision `ACTION_CMD` and count the acknowledgements.
//!
//! An action command is addressed by keys, not by camera: every device that
//! receives it compares the device key, group key and group mask with its
//! own `ActionDeviceKey`, `ActionGroupKey` and `ActionGroupMask`, and only
//! those that match act — and acknowledge. Broadcast reaches every camera on
//! the subnet; a unicast address reaches one. The scheduled form carries the
//! time, in the cameras' timestamp ticks, at which to act.

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use serde::Serialize;

use viva_gige::GVCP_PORT;
use viva_gige::{AckSummary, ActionParams, send_action};

use crate::common;

/// Send `params` to `destination` and wait `timeout_ms` for
/// acknowledgements.
pub async fn send(
    destination: Ipv4Addr,
    params: &ActionParams,
    timeout_ms: u64,
) -> Result<AckSummary> {
    send_action(
        SocketAddr::new(destination.into(), GVCP_PORT),
        params,
        timeout_ms,
    )
    .await
    .with_context(|| format!("send the action command to {destination}"))
}

#[derive(Serialize)]
struct ActionResult {
    destination: String,
    sent: usize,
    acknowledged: usize,
    refused: Vec<Refused>,
}

#[derive(Serialize)]
struct Refused {
    source: String,
    status: String,
}

/// The summary in one line: who acknowledged, and who refused and why.
pub fn describe(destination: Ipv4Addr, summary: &AckSummary, timeout_ms: u64) -> String {
    let mut line = format!(
        "action sent to {destination}; acknowledged by {} device(s) within {timeout_ms} ms",
        summary.acks
    );
    for refusal in &summary.refusals {
        line.push_str(&format!(
            "; refused by {}: {}",
            refusal.source, refusal.status
        ));
    }
    line
}

pub async fn run(
    destination: Ipv4Addr,
    params: ActionParams,
    timeout_ms: u64,
    json: bool,
) -> Result<()> {
    let summary = send(destination, &params, timeout_ms).await?;
    if json {
        return common::print_json(&ActionResult {
            destination: destination.to_string(),
            sent: summary.sent,
            acknowledged: summary.acks,
            refused: summary
                .refusals
                .iter()
                .map(|r| Refused {
                    source: r.source.to_string(),
                    status: r.status.to_string(),
                })
                .collect(),
        });
    }
    println!("{}", describe(destination, &summary, timeout_ms));
    Ok(())
}
