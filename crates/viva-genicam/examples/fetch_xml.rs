//! Fetch a camera's GenApi XML and print what the document says about itself.
//!
//! This is the layer below `connect_gige`: it opens a control channel, follows
//! the device's `FirstURL` register, and hands the bytes to `viva-genapi-xml`
//! without building a NodeMap. Useful when a camera cannot be opened, which is
//! exactly when its XML is worth reading.
//!
//! ```bash
//! cargo run -p viva-genicam --example fetch_xml
//! ```
//!
//! For a saveable copy, `viva-camctl xml --ip <CAMERA-IP> --out camera.xml`
//! does the same thing from the command line.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use viva_genapi_xml::{self, XmlError};
use viva_genicam::gige::GVCP_PORT;

fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let timeout = Duration::from_millis(500);
    let mut devices = viva_genicam::gige::discover(timeout).await?;
    if devices.is_empty() {
        println!("No cameras found.");
        return Ok(());
    }
    let device = devices.remove(0);
    println!("Connecting to {} ({})", device.ip, format_mac(&device.mac));
    let addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    let camera = Arc::new(Mutex::new(
        viva_genicam::gige::GigeDevice::open(addr).await?,
    ));

    // ANCHOR: fetch
    // `fetch_and_load_xml` knows nothing about GVCP, sockets or cameras. It
    // asks for `(address, length)` and expects bytes back, so any transport
    // that can read device memory can supply the closure.
    let xml = {
        let cam = Arc::clone(&camera);
        viva_genapi_xml::fetch_and_load_xml(move |address, length| {
            let cam = Arc::clone(&cam);
            async move {
                let mut guard = cam.lock().await;
                guard
                    .read_mem(address, length)
                    .await
                    .map_err(XmlError::transport)
            }
        })
        .await?
    };
    // ANCHOR_END: fetch
    println!("Fetched XML ({} bytes)", xml.len());

    // ANCHOR: inspect
    // A deliberately lossy parse: enough to answer "which schema is this, and
    // what is at the top level", robust to node types we do not yet handle.
    let meta = viva_genapi_xml::parse_into_minimal_nodes(&xml)?;
    if let Some(version) = meta.schema_version.as_deref() {
        println!("Schema version: {version}");
    }
    println!("Top level features ({}):", meta.top_level_features.len());
    for feature in meta.top_level_features.iter().take(8) {
        println!("  - {feature}");
    }
    // ANCHOR_END: inspect
    if meta.top_level_features.len() > 8 {
        println!("  ... ({} more)", meta.top_level_features.len() - 8);
    }

    // The full parse is what `NodeMap` is built from; `viva_genapi_xml::parse`
    // returns every node declaration, including the ones it had to skip.
    let model = viva_genapi_xml::parse(&xml)?;
    println!(
        "Full model: {} nodes, {} skipped",
        model.nodes.len(),
        model.skipped.len()
    );
    for skipped in &model.skipped {
        println!(
            "  skipped <{}> {}: {}",
            skipped.tag,
            skipped.name.as_deref().unwrap_or("(unnamed)"),
            skipped.error
        );
    }

    Ok(())
}
