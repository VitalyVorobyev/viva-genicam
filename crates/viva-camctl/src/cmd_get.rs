use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_gige::IfaceSelector;

use crate::common::{self, Target};
use crate::with_camera;

#[derive(Serialize)]
struct FeatureValue<'a> {
    name: &'a str,
    value: String,
}

pub async fn run(
    target: Target,
    name: String,
    iface: Option<IfaceSelector>,
    json: bool,
) -> Result<()> {
    info!(?target, "opening camera for get");
    let camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Monitor)
        .await
        .context("open camera for get")?;
    let value = with_camera!(&camera, camera => camera.get(&name))
        .with_context(|| format!("read feature {name}"))?;

    if json {
        let payload = FeatureValue { name: &name, value };
        common::print_json(&payload)?;
    } else {
        println!("{}", value);
    }

    Ok(())
}
