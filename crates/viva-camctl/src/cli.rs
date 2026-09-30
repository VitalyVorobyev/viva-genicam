use std::net::Ipv4Addr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use viva_gige::nic::IfaceSelector;

use crate::cmd_bench::{self, BenchArgs};
use crate::cmd_chunks;
use crate::cmd_events;
use crate::cmd_execute;
use crate::cmd_get;
use crate::cmd_list;
use crate::cmd_report::{self, ReportArgs};
use crate::cmd_set;
use crate::cmd_set_ip;
use crate::cmd_stream::{self, StreamArgs};
use crate::cmd_usb;
use crate::cmd_xml::{self, XmlArgs};

#[derive(Parser, Debug)]
#[command(name = "viva-camctl", version, about = "GenICam CLI")]
pub struct Cli {
    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,
    /// Output JSON where applicable
    #[arg(long)]
    json: bool,
    /// Host interface to use, named either by one of its IPv4 addresses
    /// (`169.254.105.106`) or by its OS name (`en0`, or a GUID on Windows)
    #[arg(long)]
    iface: Option<IfaceSelector>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Discover cameras (GVCP)
    List {
        #[arg(long, default_value_t = 1000)]
        timeout_ms: u64,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
    },
    /// Read a feature via GenApi NodeMap
    Get {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        name: String,
    },
    /// Dump the camera's GenApi XML (no parsing, so a document we cannot
    /// read still comes out)
    Xml {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        /// Write to this file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Collect a diagnostic bundle to attach to a bug report
    Report {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        /// Write the bundle here. `.txt` because GitHub rejects `.xml`
        /// attachments.
        #[arg(long, default_value = "viva-report.txt")]
        out: PathBuf,
        /// Print to stdout instead of writing a file
        #[arg(long, conflicts_with = "out")]
        stdout: bool,
        #[arg(long, default_value_t = 1000)]
        timeout_ms: u64,
        /// Leave the GenApi XML out of the bundle
        #[arg(long)]
        no_xml: bool,
    },
    /// Write a feature via GenApi NodeMap
    Set {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        value: String,
    },
    /// Execute a GenApi Command feature (e.g. `UserSetLoad`)
    Execute {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        name: String,
    },
    /// Start GVSP stream (uni-/multicast)
    Stream {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value = "unicast")]
        mode: String,
        #[arg(long)]
        group: Option<Ipv4Addr>,
        #[arg(long, default_value_t = 10040)]
        port: u16,
        /// Set GevSCPSPacketSize from the host NIC MTU, then path-probe
        /// (ADR-0021). Mutually exclusive with --packet-size. Default is to
        /// leave the camera's current value alone.
        #[arg(long, conflicts_with = "packet_size")]
        auto: bool,
        /// Override the GVSP packet size (ceiling). Default preserves the
        /// camera's GevSCPSPacketSize. Mutually exclusive with --auto.
        #[arg(long, conflicts_with = "auto")]
        packet_size: Option<u32>,
        #[arg(long, default_value_t = 1)]
        save: usize,
        /// Append every completed EVT block's raw payload to this file,
        /// in arrival order (replay capture). Image frames are unaffected;
        /// use --save for frame_NNNN files.
        #[arg(long)]
        raw_out: Option<PathBuf>,
        #[arg(long)]
        rgb: bool,
        #[arg(long, default_value_t = 0)]
        duration_s: u64,
    },
    /// Configure + read events (message channel)
    Events {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value_t = 10020)]
        port: u16,
        #[arg(long, default_value = "FrameStart,ExposureEnd")]
        enable: String,
        #[arg(long, default_value_t = 10)]
        count: u32,
    },
    /// Toggle ChunkModeActive + selectors
    Chunks {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        enable: bool,
        #[arg(long, default_value = "Timestamp")]
        selectors: String,
    },
    /// Sustained stream soak/benchmark
    Bench {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value = "unicast")]
        mode: String,
        #[arg(long)]
        group: Option<Ipv4Addr>,
        #[arg(long, default_value_t = 10040)]
        port: u16,
        #[arg(long, default_value_t = 300)]
        duration_s: u64,
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    /// Configure IP address of a GigE camera
    SetIp {
        /// MAC address (e.g. DE:AD:BE:EF:CA:FE)
        #[arg(long)]
        mac: String,
        /// IP address to assign
        #[arg(long)]
        ip: Ipv4Addr,
        /// Subnet mask
        #[arg(long, default_value = "255.255.255.0")]
        subnet: Ipv4Addr,
        /// Default gateway
        #[arg(long, default_value = "0.0.0.0")]
        gateway: Ipv4Addr,
        /// Use FORCEIP (temporary) instead of persistent registers
        #[arg(long)]
        force: bool,
    },
    /// Discover USB3 Vision cameras
    ListUsb,
    /// Read a feature from a USB3 Vision camera
    GetUsb {
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        name: String,
    },
    /// Write a feature to a USB3 Vision camera
    SetUsb {
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        value: String,
    },
    /// Stream frames from a USB3 Vision camera
    StreamUsb {
        #[arg(long)]
        index: Option<usize>,
        /// Number of frames to save to disk
        #[arg(long, default_value_t = 1)]
        save: usize,
        /// Convert saved frames to RGB
        #[arg(long)]
        rgb: bool,
        /// Stop after this many seconds (0 = unlimited)
        #[arg(long, default_value_t = 0)]
        duration_s: u64,
    },
}

/// Install the tracing subscriber for a CLI run.
///
/// `try_init` rather than `init`: when the CLI is invoked through the Python
/// wheel's console script it runs inside a process that may already have a
/// global subscriber, and a second CLI invocation in the same process must not
/// abort.
fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| level.into()),
        ))
        .with_target(false)
        .try_init();
}

/// Run the command a parsed [`Cli`] selected.
pub async fn dispatch(cli: Cli) -> Result<()> {
    let Cli {
        verbose,
        json,
        iface,
        cmd,
    } = cli;

    init_tracing(verbose);

    match cmd {
        Cmd::List {
            timeout_ms,
            iface: cmd_iface,
        } => {
            let iface = cmd_iface.or(iface);
            cmd_list::run(timeout_ms, iface, json).await?
        }
        Cmd::Get { ip, index, name } => cmd_get::run(ip, index, name, iface, json).await?,
        Cmd::Xml {
            ip,
            index,
            iface: cmd_iface,
            out,
        } => {
            let args = XmlArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                out,
            };
            cmd_xml::run(args).await?
        }
        Cmd::Report {
            ip,
            index,
            iface: cmd_iface,
            out,
            stdout,
            timeout_ms,
            no_xml,
        } => {
            let args = ReportArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                out: (!stdout).then_some(out),
                timeout_ms,
                no_xml,
            };
            cmd_report::run(args).await?
        }
        Cmd::Set {
            ip,
            index,
            name,
            value,
        } => cmd_set::run(ip, index, name, value, iface, json).await?,
        Cmd::Execute { ip, index, name } => cmd_execute::run(ip, index, name, iface, json).await?,
        Cmd::Stream {
            ip,
            index,
            iface: cmd_iface,
            mode,
            group,
            port,
            packet_size,
            auto,
            save,
            rgb,
            duration_s,
            raw_out,
        } => {
            let args = StreamArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                mode,
                group,
                port,
                packet_size,
                auto,
                save,
                rgb,
                duration_s,
                raw_out,
            };
            cmd_stream::run(args).await?
        }
        Cmd::Events {
            ip,
            index,
            iface: cmd_iface,
            port,
            enable,
            count,
        } => cmd_events::run(ip, index, cmd_iface.or(iface), port, enable, count, json).await?,
        Cmd::Chunks {
            ip,
            index,
            enable,
            selectors,
        } => cmd_chunks::run(ip, index, enable, selectors, iface, json).await?,
        Cmd::Bench {
            ip,
            index,
            iface: cmd_iface,
            mode,
            group,
            port,
            duration_s,
            json_out,
        } => {
            let args = BenchArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                mode,
                group,
                port,
                duration_s,
                json_out,
            };
            cmd_bench::run(args, json).await?
        }
        Cmd::SetIp {
            mac,
            ip,
            subnet,
            gateway,
            force,
        } => cmd_set_ip::run(&mac, ip, subnet, gateway, force, iface).await?,
        Cmd::ListUsb => cmd_usb::run_list(json)?,
        Cmd::GetUsb { index, name } => cmd_usb::run_get(index, name, json)?,
        Cmd::SetUsb { index, name, value } => cmd_usb::run_set(index, name, value, json)?,
        Cmd::StreamUsb {
            index,
            save,
            rgb,
            duration_s,
        } => cmd_usb::run_stream(index, save, rgb, duration_s)?,
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_defaults() {
        let cli = Cli::parse_from(["viva-camctl", "list"]);
        match cli.cmd {
            Cmd::List { timeout_ms, .. } => assert_eq!(timeout_ms, 1000),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_args() {
        let cli = Cli::parse_from([
            "viva-camctl",
            "stream",
            "--mode",
            "multicast",
            "--group",
            "239.1.1.1",
            "--port",
            "12000",
        ]);
        match cli.cmd {
            Cmd::Stream {
                mode, port, group, ..
            } => {
                assert_eq!(mode, "multicast");
                assert_eq!(port, 12000);
                assert_eq!(group, Some("239.1.1.1".parse().unwrap()));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn parse_bench_output_path() {
        let cli = Cli::parse_from(["viva-camctl", "bench", "--json-out", "bench.json"]);
        match cli.cmd {
            Cmd::Bench { json_out, .. } => {
                assert_eq!(json_out, Some(PathBuf::from("bench.json")));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
