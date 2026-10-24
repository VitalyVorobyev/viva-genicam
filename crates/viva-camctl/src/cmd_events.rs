use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tracing::{info, warn};

use viva_gige::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Serialize)]
struct EventRecord {
    index: usize,
    id: u16,
    ts_dev: u64,
    /// `None` without a fitted clock: the camera's ticks map to no host time.
    ts_host: Option<String>,
    payload_len: usize,
}

fn parse_events(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

/// What `events` was asked for.
#[derive(Debug, Clone)]
pub struct EventsArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<IfaceSelector>,
    /// The host port the camera sends events to.
    pub port: u16,
    /// Comma-separated `EventSelector` entries to turn on.
    pub enable: String,
    /// How many events to read.
    pub count: u32,
    /// Control or exclusive: the session configures the message channel.
    pub access: viva_genicam::Access,
}

pub async fn run(args: EventsArgs, json: bool) -> Result<()> {
    let EventsArgs {
        ip,
        index,
        iface,
        port,
        enable,
        count,
        access,
    } = args;
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface.as_ref(), timeout).await?;

    // The camera sends events *to* this address, so it must be the host's, not
    // the camera's. With no --iface, probe which local interface routes there
    // rather than refusing to run (backlog DX-08). `--iface` no longer *is*
    // the address — it may name the interface instead — so it is resolved here
    // rather than used verbatim (backlog DX-10).
    let local_ip = common::resolve_receive_iface(iface.as_ref(), device.ip)?
        .ipv4()
        .ok_or_else(|| anyhow!("interface routing to {} has no IPv4 address", device.ip))?;
    info!(ip = %device.ip, %local_ip, port, "configuring events");
    let mut camera = common::open_camera(&device, access)
        .await
        .context("open camera for events")?;

    let enable_list = parse_events(&enable);
    let enable_refs: Vec<&str> = enable_list.iter().map(|s| s.as_str()).collect();
    camera
        .configure_events(local_ip, port, &enable_refs)
        .await
        .context("configure event channel")?;
    let stream = camera
        .open_event_stream(local_ip, port)
        .await
        .context("open event stream")?;
    if let Ok(addr) = stream.local_addr() {
        info!(local = %addr, "listening for events");
    }

    let mut records = Vec::new();
    for idx in 0..usize::try_from(count).unwrap_or(0) {
        match stream.next().await {
            Ok(event) => {
                let ts_host = event
                    .ts_host
                    .and_then(|ts| common::format_system_time(ts).ok());
                if json {
                    records.push(EventRecord {
                        index: idx + 1,
                        id: event.id,
                        ts_dev: event.ts_dev,
                        ts_host,
                        payload_len: event.data.len(),
                    });
                } else {
                    println!(
                        "#{:02} host={} id=0x{:04X} ticks={} payload={} bytes",
                        idx + 1,
                        ts_host.as_deref().unwrap_or("-"),
                        event.id,
                        event.ts_dev,
                        event.data.len()
                    );
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to receive event");
                break;
            }
        }
    }

    if json {
        common::print_json(&records)?;
    }

    Ok(())
}
