use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_genapi_xml::AccessMode;
use viva_gige::nic::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

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
    ip: Option<Ipv4Addr>,
    index: Option<usize>,
    name: String,
    value: String,
    iface: Option<IfaceSelector>,
    json: bool,
) -> Result<()> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "opening camera for set");
    let mut camera = common::open_camera(&device)
        .await
        .context("open camera for set")?;
    camera
        .set(&name, &value)
        .with_context(|| format!("write feature {name}"))?;

    let access = camera.nodemap().node(&name).and_then(|n| n.access_mode());
    let read_back = if can_read_back(access) {
        Some(
            camera
                .get(&name)
                .with_context(|| format!("read feature {name}"))?,
        )
    } else {
        None
    };

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
