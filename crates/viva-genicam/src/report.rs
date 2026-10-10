//! A diagnostic report: everything we ask someone with a camera for, in one
//! plain-text file.
//!
//! Three issues, three real defects, and in every one of them the diagnosis
//! came from an artifact the reporter had to assemble by hand: a debug log, a
//! register dump, the GenApi XML. The maintainer has no cameras, so a report
//! is not a support burden — it is the only evidence this project can obtain,
//! and the cost of producing one is the rate limit on fixing bugs.
//!
//! So the report gathers all of it in one pass and keeps going when a step
//! fails: a camera we cannot open is precisely the camera worth reporting, and
//! a bundle that aborts at the first error would describe nothing. Every
//! section records either its findings or why it has none.
//!
//! `viva-camctl report`, `viva-service` and Viva Studio all write it with
//! [`build_report`], so a report reads the same whichever tool produced it
//! (backlog ST-41).

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use viva_genapi::NodeMap;
use viva_genapi_xml::{XmlError, XmlModel};
use viva_gige::gvcp::{GigeDevice, consts};
use viva_gige::nic::{Iface, IfaceSelector};
use viva_gige::{DeviceInfo, GVCP_PORT};

/// What to put in a report.
#[derive(Debug, Clone)]
pub struct ReportRequest {
    /// The tool writing the report and its version, such as
    /// `"viva-camctl 0.7.0"`; the report's first line of evidence.
    pub tool: String,
    /// Further lines for the environment section, such as the mode a viewer
    /// runs in or the camera it is connected to.
    pub notes: Vec<String>,
    /// Which camera to describe in detail: its registers, XML and features.
    pub target: ReportTarget,
    /// Discover on this interface only; `None` broadcasts on every one.
    pub iface: Option<IfaceSelector>,
    /// How long discovery waits for answers.
    pub discovery_timeout: Duration,
    /// Include the GenApi XML. It is the single most useful artifact, so a
    /// caller should leave it on unless the user asks otherwise.
    pub include_xml: bool,
}

impl ReportRequest {
    /// A report by `tool` on every camera discovery finds, describing
    /// `target` in detail, with the XML.
    pub fn new(tool: impl Into<String>, target: ReportTarget) -> Self {
        Self {
            tool: tool.into(),
            notes: Vec::new(),
            target,
            iface: None,
            discovery_timeout: Duration::from_millis(500),
            include_xml: true,
        }
    }
}

/// The camera a report describes in detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportTarget {
    /// None: the report covers the host and discovery only.
    None,
    /// The camera at this address, whether discovery saw it or not — a camera
    /// that discovery misses but that answers on a known address is itself
    /// worth reporting.
    Ip(Ipv4Addr),
    /// The `n`th camera discovery found.
    Index(usize),
}

/// How to render a register's value beside its raw hex.
///
/// The raw word is always printed — a report exists to carry facts, and a
/// decoding we got wrong must not be the only thing it says.
#[derive(Clone, Copy)]
enum Fmt {
    /// Hex only: bitmasks, where a decimal reading tells nobody anything.
    Bits,
    Dec,
    Ipv4,
}

/// Bootstrap registers worth dumping, as `(address, name, format)`.
///
/// Chosen for diagnostic value rather than completeness: the identity block,
/// the capability words, the IP configuration that #57 turned on, and the
/// channel registers whose addresses we had wrong until TC-13.
const BOOTSTRAP: &[(u64, &str, Fmt)] = &[
    (0x0000, "Version", Fmt::Bits),
    (0x0004, "DeviceMode", Fmt::Bits),
    (0x0008, "DeviceMACAddressHigh", Fmt::Bits),
    (0x000C, "DeviceMACAddressLow", Fmt::Bits),
    (0x0010, "SupportedIPConfiguration", Fmt::Bits),
    (
        consts::CURRENT_IP_CONFIG,
        "CurrentIPConfiguration",
        Fmt::Bits,
    ),
    (0x0024, "CurrentIPAddress", Fmt::Ipv4),
    (0x0034, "CurrentSubnetMask", Fmt::Ipv4),
    (0x0044, "CurrentDefaultGateway", Fmt::Ipv4),
    (
        consts::PERSISTENT_IP_ADDRESS,
        "PersistentIPAddress",
        Fmt::Ipv4,
    ),
    (
        consts::PERSISTENT_SUBNET_MASK,
        "PersistentSubnetMask",
        Fmt::Ipv4,
    ),
    (
        consts::PERSISTENT_DEFAULT_GATEWAY,
        "PersistentDefaultGateway",
        Fmt::Ipv4,
    ),
    (0x0900, "GevNumberOfMessageChannels", Fmt::Dec),
    (0x0904, "GevNumberOfStreamChannels", Fmt::Dec),
    (0x0938, "GevHeartbeatTimeout", Fmt::Dec),
    (consts::CONTROL_CHANNEL_PRIVILEGE, "GevCCP", Fmt::Bits),
    (consts::MESSAGE_DESTINATION_PORT, "GevMCP", Fmt::Dec),
    (consts::MESSAGE_DESTINATION_ADDRESS, "GevMCDA", Fmt::Ipv4),
    (consts::MESSAGE_CHANNEL_TIMEOUT, "GevMCTT", Fmt::Dec),
    (consts::MESSAGE_CHANNEL_RETRY_COUNT, "GevMCRC", Fmt::Dec),
    (
        consts::STREAM_CHANNEL_BASE + consts::STREAM_DESTINATION_PORT,
        "GevSCPHostPort[0]",
        Fmt::Dec,
    ),
    (
        consts::STREAM_CHANNEL_BASE + consts::STREAM_PACKET_SIZE,
        "GevSCPSPacketSize[0]",
        Fmt::Bits,
    ),
    (
        consts::STREAM_CHANNEL_BASE + consts::STREAM_PACKET_DELAY,
        "GevSCPD[0]",
        Fmt::Dec,
    ),
    (
        consts::STREAM_CHANNEL_BASE + consts::STREAM_DESTINATION_ADDRESS,
        "GevSCDA[0]",
        Fmt::Ipv4,
    ),
];

fn decode(value: u32, fmt: Fmt) -> String {
    match fmt {
        Fmt::Bits => String::new(),
        Fmt::Dec => value.to_string(),
        Fmt::Ipv4 => Ipv4Addr::from(value).to_string(),
    }
}

/// Build the report. It never fails: a step that cannot be done says so in
/// its section, and the rest carry on.
pub async fn build_report(request: &ReportRequest) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# viva-genicam diagnostic report");
    let _ = writeln!(out);

    environment(&mut out, request);
    interfaces(&mut out);
    let devices = discovery(
        &mut out,
        request.iface.as_ref(),
        request.discovery_timeout.as_millis() as u64,
    )
    .await;
    camera(&mut out, request, &devices).await;

    let _ = writeln!(out, "## End of report");
    out
}

fn environment(out: &mut String, request: &ReportRequest) {
    let _ = writeln!(out, "## Environment");
    let _ = writeln!(out);
    let _ = writeln!(out, "tool:        {}", request.tool);
    let _ = writeln!(
        out,
        "library:     viva-genicam {}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(
        out,
        "host:        {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    for note in &request.notes {
        let _ = writeln!(out, "{note}");
    }
    let _ = writeln!(out);
}

fn interfaces(out: &mut String) {
    let _ = writeln!(out, "## Network interfaces");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "As the library sees them. An interface missing here is invisible to \
         discovery no matter what the OS reports elsewhere (#57)."
    );
    let _ = writeln!(out);
    // `mtu()` only queries the OS on Linux and returns 1500 everywhere else
    // (TC-11). Printing that as though it had been measured would put a
    // fabricated number in a document whose whole purpose is evidence.
    let mtu_measured = cfg!(target_os = "linux");
    match Iface::list() {
        Ok(ifaces) if ifaces.is_empty() => {
            let _ = writeln!(out, "(none reported)");
        }
        Ok(ifaces) => {
            for iface in ifaces {
                let addrs = iface.all_ipv4().unwrap_or_default();
                let addrs = if addrs.is_empty() {
                    "-".to_string()
                } else {
                    addrs
                        .iter()
                        .map(|ip| ip.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let mtu = if mtu_measured {
                    viva_gige::nic::mtu(&iface)
                        .map(|mtu| mtu.to_string())
                        .unwrap_or_else(|err| format!("unknown ({err})"))
                } else {
                    "assumed".to_string()
                };
                let _ = writeln!(
                    out,
                    "{:<16} index={:<5} mtu={:<9} ipv4=[{}]",
                    iface.name(),
                    iface.index(),
                    mtu,
                    addrs
                );
            }
            if !mtu_measured {
                let _ = writeln!(
                    out,
                    "\nMTU is not queried on {} — the library assumes 1500 and \
                     cannot select jumbo frames here (TC-11).",
                    std::env::consts::OS
                );
            }
        }
        Err(err) => {
            let _ = writeln!(out, "FAILED to enumerate interfaces: {err}");
        }
    }
    let _ = writeln!(out);
}

async fn discover_devices(
    timeout: Duration,
    iface: Option<&IfaceSelector>,
) -> Result<Vec<DeviceInfo>, String> {
    match iface {
        Some(selector) => {
            let iface = selector
                .resolve()
                .map_err(|e| format!("resolve host interface '{selector}': {e}"))?;
            viva_gige::discover_on_interface(timeout, iface.name())
                .await
                .map_err(|e| format!("discover devices on interface: {e}"))
        }
        None => viva_gige::discover(timeout)
            .await
            .map_err(|e| format!("broadcast discovery: {e}")),
    }
}

async fn discovery(
    out: &mut String,
    iface: Option<&IfaceSelector>,
    timeout_ms: u64,
) -> Vec<DeviceInfo> {
    let _ = writeln!(out, "## Discovery");
    let _ = writeln!(out);
    let timeout = Duration::from_millis(timeout_ms);
    match discover_devices(timeout, iface).await {
        Ok(devices) if devices.is_empty() => {
            let _ = writeln!(out, "no cameras answered within {timeout_ms} ms");
            let _ = writeln!(out);
            Vec::new()
        }
        Ok(devices) => {
            for (index, dev) in devices.iter().enumerate() {
                let _ = writeln!(out, "[{index}] {}", dev.ip);
                let _ = writeln!(out, "     mac:          {}", dev.mac_string());
                let _ = writeln!(out, "     manufacturer: {}", opt(&dev.manufacturer));
                let _ = writeln!(out, "     model:        {}", opt(&dev.model));
                let _ = writeln!(out, "     version:      {}", opt(&dev.version));
                let _ = writeln!(out, "     serial:       {}", opt(&dev.serial));
                let _ = writeln!(out, "     user name:    {}", opt(&dev.user_name));
            }
            let _ = writeln!(out);
            devices
        }
        Err(err) => {
            let _ = writeln!(out, "FAILED: {err}");
            let _ = writeln!(out);
            Vec::new()
        }
    }
}

async fn camera(out: &mut String, request: &ReportRequest, discovered: &[DeviceInfo]) {
    let _ = writeln!(out, "## Camera");
    let _ = writeln!(out);

    let device = match select(request.target, discovered) {
        Some(device) => device,
        None => {
            let _ = writeln!(
                out,
                "No camera selected, so there are no register, XML or feature sections."
            );
            let _ = writeln!(out);
            return;
        }
    };
    let _ = writeln!(out, "selected: {}", device.ip);
    let _ = writeln!(out);

    let addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    let control = match GigeDevice::open(addr).await {
        Ok(control) => Arc::new(Mutex::new(control)),
        Err(err) => {
            let _ = writeln!(
                out,
                "FAILED to open the control channel at {}: {err}",
                device.ip
            );
            let _ = writeln!(out);
            return;
        }
    };

    bootstrap_registers(out, &control).await;

    let xml = match fetch_xml(Arc::clone(&control)).await {
        Ok(xml) => xml,
        Err(err) => {
            let _ = writeln!(out, "## GenApi XML");
            let _ = writeln!(out);
            let _ = writeln!(out, "FAILED to fetch: {err}");
            let _ = writeln!(out);
            return;
        }
    };
    genapi(out, &xml, request.include_xml);
}

async fn fetch_xml(control: Arc<Mutex<GigeDevice>>) -> Result<String, XmlError> {
    viva_genapi_xml::fetch_and_load_xml(move |address, length| {
        let control = Arc::clone(&control);
        async move {
            let mut guard = control.lock().await;
            guard
                .read_mem(address, length)
                .await
                .map_err(|err| XmlError::Transport(err.to_string()))
        }
    })
    .await
}

fn select(target: ReportTarget, discovered: &[DeviceInfo]) -> Option<DeviceInfo> {
    match target {
        ReportTarget::None => None,
        ReportTarget::Ip(ip) => Some(
            discovered
                .iter()
                .find(|dev| dev.ip == ip)
                .cloned()
                .unwrap_or_else(|| DeviceInfo::from_ip(ip)),
        ),
        ReportTarget::Index(index) => discovered.get(index).cloned(),
    }
}

async fn bootstrap_registers(out: &mut String, control: &Arc<Mutex<GigeDevice>>) {
    let _ = writeln!(out, "## Bootstrap registers");
    let _ = writeln!(out);
    let mut guard = control.lock().await;
    for (addr, name, fmt) in BOOTSTRAP {
        let value = match guard.read_register(*addr as u32).await {
            Ok(value) => format!("0x{value:08X}  {}", decode(value, *fmt)),
            Err(err) => format!("read failed: {err}"),
        };
        let _ = writeln!(out, "0x{addr:04X}  {name:<26} {}", value.trim_end());
    }
    let _ = writeln!(out);
}

fn genapi(out: &mut String, xml: &str, include_xml: bool) {
    let _ = writeln!(out, "## GenApi");
    let _ = writeln!(out);
    let _ = writeln!(out, "XML size: {} bytes", xml.len());

    let model: Option<XmlModel> = match viva_genapi_xml::parse(xml) {
        Ok(model) => {
            let _ = writeln!(
                out,
                "schema:   {}\nparsed:   {} nodes",
                model.version,
                model.nodes.len()
            );
            Some(model)
        }
        Err(err) => {
            let _ = writeln!(out, "PARSE FAILED: {err}");
            None
        }
    };

    if let Some(model) = model {
        match NodeMap::try_from_xml(model) {
            Ok(nodemap) => {
                let _ = writeln!(out, "built:    {} features", nodemap.node_names().count());
                let _ = writeln!(out);
                let skipped = nodemap.skipped();
                if skipped.is_empty() {
                    let _ = writeln!(out, "No features were dropped.");
                } else {
                    let _ = writeln!(
                        out,
                        "{} feature(s) this camera has that we cannot expose — \
                         these are the interesting ones:",
                        skipped.len()
                    );
                    let _ = writeln!(out);
                    for node in skipped {
                        let _ = writeln!(
                            out,
                            "  <{}> {}: {}",
                            node.tag,
                            node.name.as_deref().unwrap_or("<unnamed>"),
                            node.error
                        );
                    }
                }
            }
            Err(err) => {
                let _ = writeln!(out, "NODEMAP BUILD FAILED: {err}");
            }
        }
    }
    let _ = writeln!(out);

    if !include_xml {
        let _ = writeln!(
            out,
            "## GenApi XML\n\nOmitted on request. Include it, or run \
             `viva-camctl xml`, if asked for the document."
        );
    } else {
        let _ = writeln!(out, "## GenApi XML");
        let _ = writeln!(out);
        let _ = writeln!(out, "{xml}");
    }
    let _ = writeln!(out);
}

fn opt(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_addresses_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (addr, name, _) in BOOTSTRAP {
            assert!(seen.insert(*addr), "duplicate address for {name}");
        }
    }

    #[test]
    fn ip_registers_decode_to_dotted_quads() {
        assert_eq!(decode(0x7F00_0001, Fmt::Ipv4), "127.0.0.1");
        assert_eq!(decode(0xFF00_0000, Fmt::Ipv4), "255.0.0.0");
        assert_eq!(decode(3000, Fmt::Dec), "3000");
        assert_eq!(decode(0x8000_0001, Fmt::Bits), "");
    }

    /// A camera we cannot understand is the one worth reporting, so the
    /// GenApi section must describe the failure rather than abort the report.
    #[test]
    fn unparsable_xml_still_produces_a_section() {
        let mut out = String::new();
        genapi(&mut out, "<not-genicam>", false);
        assert!(out.contains("PARSE FAILED"), "{out}");
        assert!(out.contains("XML size: 13 bytes"), "{out}");
    }

    #[test]
    fn skipped_features_are_listed() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <ConfRom Name="DeviceConfRom">
                    <Address>0x2000</Address>
                    <Length>512</Length>
                </ConfRom>
            </RegisterDescription>
        "#;
        let mut out = String::new();
        genapi(&mut out, XML, false);
        assert!(out.contains("<ConfRom> DeviceConfRom"), "{out}");
        assert!(out.contains("cannot expose"), "{out}");
    }

    #[test]
    fn the_xml_is_included_unless_opted_out() {
        const XML: &str = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0"/>"#;
        let mut with = String::new();
        genapi(&mut with, XML, true);
        assert!(with.contains("SchemaMajorVersion"), "{with}");

        let mut without = String::new();
        genapi(&mut without, XML, false);
        assert!(without.contains("Omitted on request"), "{without}");
    }
}
