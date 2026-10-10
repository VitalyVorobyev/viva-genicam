//! `viva-camctl report` — one command that produces everything we ask for.
//!
//! The report itself is [`viva_genicam::report`], which `viva-service` and
//! Viva Studio write too; this command picks the camera and the file.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use viva_genicam::report::{ReportRequest, ReportTarget, build_report};
use viva_gige::nic::IfaceSelector;

pub struct ReportArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    pub iface: Option<IfaceSelector>,
    pub out: Option<PathBuf>,
    pub timeout_ms: u64,
    /// Omit the GenApi XML. It is the single most useful artifact, so this is
    /// opt-out rather than opt-in.
    pub no_xml: bool,
}

pub async fn run(args: ReportArgs) -> Result<()> {
    let target = match (args.ip, args.index) {
        (Some(ip), _) => ReportTarget::Ip(ip),
        (None, Some(index)) => ReportTarget::Index(index),
        (None, None) => ReportTarget::None,
    };
    let request = ReportRequest {
        iface: args.iface,
        discovery_timeout: Duration::from_millis(args.timeout_ms),
        include_xml: !args.no_xml,
        ..ReportRequest::new(format!("viva-camctl {}", env!("CARGO_PKG_VERSION")), target)
    };
    let out = build_report(&request).await;

    match args.out {
        Some(path) => {
            std::fs::write(&path, out.as_bytes())
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!(
                "wrote {} bytes to {}\n\nAttach this file to \
                 https://github.com/VitalyVorobyev/viva-genicam/issues",
                out.len(),
                path.display()
            );
        }
        None => print!("{out}"),
    }
    Ok(())
}
