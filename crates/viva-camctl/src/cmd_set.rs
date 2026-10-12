use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_genapi_xml::AccessMode;
use viva_gige::IfaceSelector;

use crate::common::{self, Target};
use crate::with_camera;

#[derive(Serialize)]
struct SetResponse<'a> {
    name: &'a str,
    /// The value read back after the write; `None` for a write-only node.
    value: Option<String>,
    write_only: bool,
}

/// Whether a node's value can be read back after writing it.
///
/// A write-only node refuses the read, so reading it back used to report a
/// failure after the write had succeeded — and on a real camera it put a
/// rejected request on the wire as well (backlog DX-11, found tracing #135).
fn can_read_back(access: Option<AccessMode>) -> bool {
    matches!(access, Some(AccessMode::RO | AccessMode::RW))
}

pub async fn run(
    target: Target,
    name: String,
    value: String,
    iface: Option<IfaceSelector>,
    json: bool,
) -> Result<()> {
    info!(?target, "opening camera for set");
    let mut camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Control)
        .await
        .context("open camera for set")?;
    let read_back =
        with_camera!(&mut camera, camera => write_and_read_back(camera, &name, &value))?;

    if json {
        let payload = SetResponse {
            name: &name,
            write_only: read_back.is_none(),
            value: read_back,
        };
        common::print_json(&payload)?;
    } else {
        match read_back {
            Some(value) => println!("{value}"),
            None => println!("wrote {name} = {value} (write-only node; not read back)"),
        }
    }

    Ok(())
}

/// Write `name`, then read it back unless it is write-only.
fn write_and_read_back<T: viva_genicam::genapi::RegisterIo>(
    camera: &mut viva_genicam::Camera<T>,
    name: &str,
    value: &str,
) -> Result<Option<String>> {
    camera
        .set(name, value)
        .with_context(|| format!("write feature {name}"))?;
    let access = camera.nodemap().node(name).and_then(|n| n.access_mode());
    if !can_read_back(access) {
        return Ok(None);
    }
    camera
        .get(name)
        .map(Some)
        .with_context(|| format!("read feature {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_readable_nodes_are_read_back() {
        assert!(can_read_back(Some(AccessMode::RW)));
        assert!(can_read_back(Some(AccessMode::RO)));
        assert!(!can_read_back(Some(AccessMode::WO)));
        // A category has no access mode and no value to show.
        assert!(!can_read_back(None));
    }
}
