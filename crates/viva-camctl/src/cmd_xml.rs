//! `viva-camctl xml` — dump a camera's GenApi XML.
//!
//! Every camera-specific bug found so far was diagnosed from the reporter's
//! own XML, and until now the library offered no supported way to produce one:
//! the fetch existed but only behind [`common::open_camera`], which builds a
//! nodemap first. That is exactly the step that fails on the cameras whose XML
//! we most need — the reporter of issue #45 was told camctl could dump it,
//! could not, and had to supply four other models instead.
//!
//! So this command stops at the fetch. Nothing is parsed, so nothing about the
//! document's contents can make it fail.
//!
//! One step is left that can: a camera usually serves the document as a ZIP
//! archive, which is unpacked. `--raw` skips that too and writes the bytes as
//! served; issue #236 was an archive the reader refused, which its reporter
//! had to read out of the camera by other means.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::info;

use viva_gige::IfaceSelector;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS};

pub struct XmlArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<IfaceSelector>,
    pub out: Option<PathBuf>,
    pub raw: bool,
}

pub async fn run(args: XmlArgs) -> Result<()> {
    let XmlArgs {
        ip,
        index,
        iface,
        out,
        raw,
    } = args;
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(ip, index, iface.as_ref(), timeout).await?;
    info!(ip = %device.ip, "fetching GenApi XML");

    let control = Arc::new(Mutex::new(common::open_control(&device).await?));
    if raw {
        // clap requires `--out` with `--raw`: the bytes may be an archive.
        let path = out.context("--raw needs --out")?;
        let bytes = common::fetch_raw_xml(control).await?;
        std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
        eprintln!("wrote {} bytes to {}", bytes.len(), path.display());
        return Ok(());
    }
    let xml = common::fetch_xml(control).await?;

    match out {
        Some(path) => {
            std::fs::write(&path, xml.as_bytes())
                .with_context(|| format!("write {}", path.display()))?;
            // On stderr so `--out /dev/stdout` still yields a clean document.
            eprintln!("wrote {} bytes to {}", xml.len(), path.display());
        }
        None => print!("{xml}"),
    }
    Ok(())
}
