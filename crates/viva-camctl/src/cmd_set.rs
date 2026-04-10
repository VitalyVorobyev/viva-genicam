use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Serialize)]
struct SetResponse<'a> {
    name: &'a str,
    value: String,
}

pub async fn run(
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    name: String,
    value: String,
    iface: Option<Ipv4Addr>,
    json: bool,
) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface, timeout).await?;
    info!(ip = %device.ip, "opening camera for set");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for set")?;
    camera
        .set(&name, &value)
        .with_context(|| format!("write feature {name}"))?;
    let read_back = camera
        .get(&name)
        .with_context(|| format!("read feature {name}"))?;

    if json {
        let payload = SetResponse {
            name: &name,
            value: read_back,
        };
        common::print_json(&payload)?;
    } else {
        println!("{}", read_back);
    }

    Ok(())
}
