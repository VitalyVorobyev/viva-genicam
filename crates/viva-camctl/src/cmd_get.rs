use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_gige::nic::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

#[derive(Serialize)]
struct FeatureValue<'a> {
    name: &'a str,
    value: String,
}

pub async fn run(
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    name: String,
    iface: Option<IfaceSelector>,
    json: bool,
) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for get");
    let camera = common::open_camera(&device, viva_genicam::Access::Monitor)
        .await
        .context("open camera for get")?;
    let value = camera
        .get(&name)
        .with_context(|| format!("read feature {name}"))?;

    if json {
        let payload = FeatureValue { name: &name, value };
        common::print_json(&payload)?;
    } else {
        println!("{}", value);
    }

    Ok(())
}
