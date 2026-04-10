use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Serialize)]
struct ChunkStatus {
    active: bool,
    selectors: Vec<String>,
}

fn parse_selectors(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

pub async fn run(
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    enable: bool,
    selectors: String,
    iface: Option<Ipv4Addr>,
    json: bool,
) -> Result<()> {
    let selected = parse_selectors(&selectors);
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface, timeout).await?;
    info!(ip = %device.ip, enable, "configuring chunk mode");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for chunk configuration")?;

    let cfg: viva_genicam::ChunkConfig = viva_genicam::ChunkConfig {
        selectors: selected.clone(),
        active: enable,
    };
    camera
        .configure_chunks(&cfg)
        .context("enable/disable chunk selectors")?;

    if json {
        let status = ChunkStatus {
            active: enable,
            selectors: selected.clone(),
        };
        common::print_json(&status)?;
    } else {
        let summary = if selected.is_empty() {
            "no selectors".to_string()
        } else {
            selected.join(", ")
        };
        println!(
            "Chunk mode {} ({})",
            if enable { "enabled" } else { "disabled" },
            summary
        );
    }

    Ok(())
}
