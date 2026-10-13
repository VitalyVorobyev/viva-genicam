//! `viva-camctl report` — one command that produces everything we ask for.
//!
//! The report itself is [`viva_genicam::report`], which `viva-service` and
//! Viva Studio write too; this command picks the camera and the file.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use viva_genicam::report::{OpenCamera, ReportRequest, ReportTarget, build_report};
use viva_gige::IfaceSelector;

pub struct ReportArgs {
    pub ip: Option<Ipv4Addr>,
    pub index: Option<usize>,
    /// The Nth USB3 Vision camera, in `list-usb` order.
    pub usb: Option<usize>,
    pub iface: Option<IfaceSelector>,
    pub out: Option<PathBuf>,
    pub timeout_ms: u64,
    /// Omit the GenApi XML. It is the single most useful artifact, so this is
    /// opt-out rather than opt-in.
    pub no_xml: bool,
}

pub async fn run(args: ReportArgs) -> Result<()> {
    let mut notes = Vec::new();
    let target = match (args.ip, args.index, args.usb) {
        (Some(ip), _, _) => ReportTarget::Ip(ip),
        (None, Some(index), _) => ReportTarget::Index(index),
        (None, None, Some(usb)) => {
            match tokio::task::spawn_blocking(move || open_usb(usb)).await? {
                Ok(camera) => ReportTarget::Open(camera),
                // A camera we cannot open is the one worth reporting: say why,
                // and report the rest (backlog DX-06).
                Err(err) => {
                    notes.push(format!(
                        "usb camera [{usb}]: FAILED to open: {}",
                        crate::common::error_line(&err)
                    ));
                    ReportTarget::None
                }
            }
        }
        (None, None, None) => ReportTarget::None,
    };
    let request = ReportRequest {
        notes,
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

/// Open the `index`th USB3 Vision camera and read what the report needs. The
/// report cannot open it itself: a USB device admits one owner, so whoever
/// holds it hands over its identity and XML (DX-06).
///
/// The bootstrap registers go in raw as well as decoded: one USB3 Vision
/// camera has been checked against this library (backlog TC-27), so a
/// report is what settles how another's registers really read. A camera whose
/// XML cannot be read is still described.
fn open_usb(index: usize) -> Result<OpenCamera> {
    let devices = viva_u3v::discover().context("USB3 Vision discovery")?;
    let info = devices
        .get(index)
        .with_context(|| format!("no USB3 Vision camera [{index}]; {} found", devices.len()))?;
    let opt = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
    let mut identity = vec![
        (
            "usb".to_string(),
            format!(
                "{:04x}:{:04x} on bus {} address {}",
                info.vendor_id, info.product_id, info.bus, info.address
            ),
        ),
        ("manufacturer".to_string(), opt(&info.manufacturer)),
        ("model".to_string(), opt(&info.model)),
        ("serial".to_string(), opt(&info.serial)),
    ];
    let mut device =
        viva_u3v::U3vDevice::open_device(info).context("open the USB3 Vision camera")?;
    identity.extend(device.describe());
    let xml = device.fetch_xml().unwrap_or_else(|err| {
        identity.push(("XML".to_string(), format!("FAILED to read: {err}")));
        String::new()
    });
    Ok(OpenCamera {
        transport: "USB3 Vision".to_string(),
        identity,
        xml,
    })
}
