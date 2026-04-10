use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::{info, warn};

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Serialize)]
struct EventRecord {
    index: usize,
    id: u16,
    ts_dev: u64,
    ts_host: String,
    payload_len: usize,
}

fn parse_events(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

pub async fn run(
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    iface: Ipv4Addr,
    port: u16,
    enable: String,
    count: u32,
    json: bool,
) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, Some(iface), timeout).await?;
    info!(ip = %device.ip, port, "configuring events");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for events")?;

    let enable_list = parse_events(&enable);
    let enable_refs: Vec<&str> = enable_list.iter().map(|s| s.as_str()).collect();
    camera
        .configure_events(iface, port, &enable_refs)
        .await
        .context("configure event channel")?;
    let stream = camera
        .open_event_stream(iface, port)
        .await
        .context("open event stream")?;
    if let Ok(addr) = stream.local_addr() {
        info!(local = %addr, "listening for events");
    }

    let mut records = Vec::new();
    for idx in 0..usize::try_from(count).unwrap_or(0) {
        match stream.next().await {
            Ok(event) => {
                let ts_host = common::format_system_time(event.ts_host)
                    .unwrap_or_else(|_| "unknown".to_string());
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
                        ts_host,
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
