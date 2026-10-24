use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use viva_gige::IfaceSelector;

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
use crate::common::{self, AccessArg, SaveFormat, Target};
use crate::{cmd_action, cmd_config, cmd_inspect, cmd_registers};
use viva_gige::ActionParams;

/// A 32-bit value, decimal or `0x` hexadecimal.
fn parse_word(text: &str) -> Result<u32, String> {
    let value = common::parse_number(text)?;
    u32::try_from(value).map_err(|_| format!("{text} does not fit in 32 bits"))
}

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
        /// Ask the camera at this address directly (unicast), for one that
        /// broadcast discovery does not reach
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        /// Let cameras broadcast their answer, for one configured for another
        /// subnet that cannot answer the host directly
        #[arg(long)]
        allow_broadcast_ack: bool,
    },
    /// Read a feature, formatted by its type
    Get {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        name: String,
    },
    /// The category tree: every feature with its type, access and value
    Features {
        #[command(flatten)]
        target: Target,
        /// Only features whose name contains this, ignoring case
        #[arg(long)]
        filter: Option<String>,
        /// Include the features the camera marks invisible
        #[arg(long)]
        all: bool,
    },
    /// Everything about one feature: documentation, access, value, range,
    /// entries, selectors, and what it depends on
    Describe {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        name: String,
    },
    /// Every value the camera has to read now, in XML order
    Dump {
        #[command(flatten)]
        target: Target,
    },
    /// Save the camera's configuration (its streamable features) to a file
    Save {
        #[command(flatten)]
        target: Target,
        /// The file to write
        #[arg(long)]
        out: PathBuf,
    },
    /// Restore a configuration saved with `save` (or another GenApi tool)
    Load {
        #[command(flatten)]
        target: Target,
        /// The file to read
        #[arg(long)]
        file: PathBuf,
    },
    /// Read device registers directly, past the GenApi XML
    ReadReg {
        #[command(flatten)]
        target: Target,
        /// Register address, decimal or 0x-hexadecimal
        #[arg(long, value_parser = common::parse_number)]
        addr: u64,
        /// Bytes to read; a multiple of 4 on GigE Vision
        #[arg(long, default_value_t = 4)]
        len: usize,
    },
    /// Write one 32-bit device register directly, past the GenApi XML
    WriteReg {
        #[command(flatten)]
        target: Target,
        /// Register address, decimal or 0x-hexadecimal
        #[arg(long, value_parser = common::parse_number)]
        addr: u64,
        /// The 32-bit value, decimal or 0x-hexadecimal, in the device's byte order
        #[arg(long, value_parser = parse_word)]
        value: u32,
    },
    /// Send a GigE Vision action command and count the acknowledgements
    Action {
        /// Where to send it [default: broadcast, 255.255.255.255]
        #[arg(long, default_value_t = Ipv4Addr::BROADCAST)]
        destination: Ipv4Addr,
        /// ActionDeviceKey
        #[arg(long, value_parser = parse_word)]
        device_key: u32,
        /// ActionGroupKey
        #[arg(long, value_parser = parse_word)]
        group_key: u32,
        /// ActionGroupMask
        #[arg(long, value_parser = parse_word)]
        group_mask: u32,
        /// Act at this time, in the cameras' timestamp ticks (scheduled action)
        #[arg(long, value_parser = common::parse_number)]
        scheduled_time: Option<u64>,
        /// How long to collect acknowledgements
        #[arg(long, default_value_t = 200)]
        timeout_ms: u64,
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
        /// Describe the Nth USB3 Vision camera (`list-usb` order) instead
        #[arg(long, conflicts_with_all = ["ip", "index"])]
        usb: Option<usize>,
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
    /// Write a feature from text, parsed by its type
    Set {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        name: String,
        #[arg(long)]
        value: String,
    },
    /// Execute a GenApi Command feature (e.g. `UserSetLoad`) and wait until
    /// the camera reports it finished
    Execute {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        name: String,
        /// How long to wait for the camera to report the command finished
        /// (GenICam `IsDone`)
        #[arg(long, default_value_t = 5000)]
        wait_ms: u64,
        /// Return once the write is acknowledged, without waiting for the
        /// command to finish
        #[arg(long, conflicts_with = "wait_ms")]
        no_wait: bool,
    },
    /// Stream frames: GVSP (uni-/multicast), or a USB3 Vision camera with
    /// --usb
    Stream {
        #[arg(long, conflicts_with = "usb")]
        ip: Option<Ipv4Addr>,
        #[arg(long, conflicts_with = "usb")]
        index: Option<usize>,
        /// Stream from the Nth USB3 Vision camera (`list-usb` order) instead
        #[arg(long)]
        usb: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value = "unicast")]
        mode: String,
        #[arg(long)]
        group: Option<Ipv4Addr>,
        /// Receive on this UDP port, for a firewall that admits only it
        /// [default: a free port the operating system picks]
        #[arg(long)]
        port: Option<u16>,
        /// Have the camera wait this many nanoseconds between packets
        /// (GevSCPD, converted to its timestamp ticks) [default: leave
        /// GevSCPD as it is]
        #[arg(long)]
        packet_delay_ns: Option<u64>,
        /// Never ask the camera to resend lost packets [default: ask, if the
        /// camera supports it]
        #[arg(long)]
        no_resend: bool,
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
        /// File format for --save: pnm (8-bit PGM/PPM), or png/tiff at the
        /// camera's bit depth (16-bit above 8 bits)
        #[arg(long, value_enum, default_value_t = SaveFormat::Pnm)]
        save_format: SaveFormat,
        /// Append every completed EVT block's raw payload to this file,
        /// in arrival order (replay capture). Image frames are unaffected;
        /// use --save for frame_NNNN files.
        #[arg(long)]
        raw_out: Option<PathBuf>,
        #[arg(long)]
        rgb: bool,
        #[arg(long, default_value_t = 0)]
        duration_s: u64,
        /// How to open the camera [default: control]
        #[arg(long, value_enum, conflicts_with = "monitor")]
        access: Option<AccessArg>,
        /// Watch the multicast stream another application set up: open
        /// read-only (`--access monitor`), read the group, port and packet
        /// size from the camera, and start and stop nothing
        #[arg(long, conflicts_with_all = ["usb", "auto", "packet_size", "packet_delay_ns", "port", "group"])]
        monitor: bool,
    },
    /// Configure + read events: the GVCP message channel, or a USB3 Vision
    /// camera's event interface with --usb (which streams while it listens)
    Events {
        #[arg(long, conflicts_with = "usb")]
        ip: Option<Ipv4Addr>,
        #[arg(long, conflicts_with = "usb")]
        index: Option<usize>,
        /// Read the Nth USB3 Vision camera's events (`list-usb` order)
        /// instead
        #[arg(long)]
        usb: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value_t = 10020)]
        port: u16,
        #[arg(long, default_value = "FrameStart,ExposureEnd")]
        enable: String,
        #[arg(long, default_value_t = 10)]
        count: u32,
        /// How to open the camera: control or exclusive [default: control].
        /// Not monitor: the camera sends events to the one destination its
        /// controller sets
        #[arg(long, value_enum)]
        access: Option<AccessArg>,
    },
    /// Toggle ChunkModeActive + selectors
    Chunks {
        #[arg(long, conflicts_with = "usb")]
        ip: Option<Ipv4Addr>,
        #[arg(long, conflicts_with = "usb")]
        index: Option<usize>,
        /// Configure the Nth USB3 Vision camera (`list-usb` order) instead
        #[arg(long)]
        usb: Option<usize>,
        #[arg(long)]
        enable: bool,
        #[arg(long, default_value = "Timestamp")]
        selectors: String,
        /// How to open the camera: control or exclusive [default: control].
        /// Not monitor: configuring chunks writes
        #[arg(long, value_enum)]
        access: Option<AccessArg>,
    },
    /// Sustained stream soak/benchmark
    Bench {
        #[arg(long, conflicts_with = "usb")]
        ip: Option<Ipv4Addr>,
        #[arg(long, conflicts_with = "usb")]
        index: Option<usize>,
        /// Benchmark the Nth USB3 Vision camera (`list-usb` order) instead
        #[arg(long)]
        usb: Option<usize>,
        /// Host interface, by IPv4 address or by OS name
        #[arg(long)]
        iface: Option<IfaceSelector>,
        #[arg(long, default_value = "unicast")]
        mode: String,
        #[arg(long)]
        group: Option<Ipv4Addr>,
        /// Receive on this UDP port, for a firewall that admits only it
        /// [default: a free port the operating system picks]
        #[arg(long)]
        port: Option<u16>,
        /// Have the camera wait this many nanoseconds between packets
        /// (GevSCPD, converted to its timestamp ticks) [default: leave
        /// GevSCPD as it is]
        #[arg(long)]
        packet_delay_ns: Option<u64>,
        /// Never ask the camera to resend lost packets [default: ask, if the
        /// camera supports it]
        #[arg(long)]
        no_resend: bool,
        #[arg(long, default_value_t = 300)]
        duration_s: u64,
        #[arg(long)]
        json_out: Option<PathBuf>,
        /// How to open the camera [default: control]
        #[arg(long, value_enum, conflicts_with = "monitor")]
        access: Option<AccessArg>,
        /// Benchmark the multicast stream another application set up, as
        /// `stream --monitor` watches it
        #[arg(long, conflicts_with_all = ["usb", "packet_delay_ns", "port", "group"])]
        monitor: bool,
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
}

/// Install the tracing subscriber for a CLI run.
///
/// Logs go to stderr: stdout carries a command's data, which is piped or
/// redirected into files (`dump > values.txt`, `xml > camera.xml`).
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
    // nusb logs its own event-loop thread at info, for every USB command.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| format!("{level},nusb=warn")),
        ))
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

/// The access `stream` and `bench` open a GigE Vision camera with:
/// `--monitor` is `--access monitor`, and control is the default.
fn stream_access(access: Option<AccessArg>, monitor: bool) -> viva_genicam::Access {
    match (access, monitor) {
        (_, true) => viva_genicam::Access::Monitor,
        (Some(access), false) => access.into(),
        (None, false) => viva_genicam::Access::Control,
    }
}

/// The access a command that writes opens with: control by default, or
/// exclusive, and never a monitor.
fn controlling_access(
    access: Option<AccessArg>,
    command: &str,
    usb: bool,
) -> Result<viva_genicam::Access> {
    if access == Some(AccessArg::Monitor) {
        anyhow::bail!(
            "{command} configures the camera, and a monitor (--access monitor) writes nothing"
        );
    }
    common::chosen_access(access, viva_genicam::Access::Control, usb)
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
            ip,
            allow_broadcast_ack,
        } => {
            let iface = cmd_iface.or(iface);
            let args = cmd_list::ListArgs {
                timeout_ms,
                iface,
                ip,
                allow_broadcast_ack,
            };
            cmd_list::run(args, json).await?
        }
        Cmd::Get { target, name } => cmd_get::run(target, name, iface, json).await?,
        Cmd::Features {
            target,
            filter,
            all,
        } => cmd_inspect::run_features(target, iface, filter, all, json).await?,
        Cmd::Describe { target, name } => {
            cmd_inspect::run_describe(target, iface, name, json).await?
        }
        Cmd::Dump { target } => cmd_inspect::run_dump(target, iface, json).await?,
        Cmd::Save { target, out } => cmd_config::run_save(target, iface, &out, json).await?,
        Cmd::Load { target, file } => cmd_config::run_load(target, iface, &file, json).await?,
        Cmd::ReadReg { target, addr, len } => {
            cmd_registers::run_read(target, iface, addr, len, json).await?
        }
        Cmd::WriteReg {
            target,
            addr,
            value,
        } => cmd_registers::run_write(target, iface, addr, value).await?,
        Cmd::Action {
            destination,
            device_key,
            group_key,
            group_mask,
            scheduled_time,
            timeout_ms,
        } => {
            let params = ActionParams {
                device_key,
                group_key,
                group_mask,
                scheduled_time,
            };
            cmd_action::run(destination, params, timeout_ms, json).await?
        }
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
            usb,
            iface: cmd_iface,
            out,
            stdout,
            timeout_ms,
            no_xml,
        } => {
            let args = ReportArgs {
                ip,
                index,
                usb,
                iface: cmd_iface.or(iface),
                out: (!stdout).then_some(out),
                timeout_ms,
                no_xml,
            };
            cmd_report::run(args).await?
        }
        Cmd::Set {
            target,
            name,
            value,
        } => cmd_set::run(target, name, value, iface, json).await?,
        Cmd::Execute {
            target,
            name,
            wait_ms,
            no_wait,
        } => {
            let wait = (!no_wait).then(|| Duration::from_millis(wait_ms));
            cmd_execute::run(target, name, wait, iface, json).await?
        }
        Cmd::Stream {
            usb: Some(usb),
            save,
            save_format,
            rgb,
            duration_s,
            access,
            ..
        } => {
            common::chosen_access(access, viva_genicam::Access::Control, true)?;
            cmd_usb::run_stream(usb, save, save_format, rgb, duration_s)?
        }
        Cmd::Stream {
            ip,
            index,
            usb: None,
            iface: cmd_iface,
            mode,
            group,
            port,
            packet_delay_ns,
            no_resend,
            packet_size,
            auto,
            save,
            save_format,
            rgb,
            duration_s,
            raw_out,
            access,
            monitor,
        } => {
            let args = StreamArgs {
                access: stream_access(access, monitor),
                ip,
                index,
                iface: cmd_iface.or(iface),
                mode,
                group,
                port,
                packet_delay: packet_delay_ns.map(Duration::from_nanos),
                resend: !no_resend,
                packet_size,
                auto,
                save,
                save_format,
                rgb,
                duration_s,
                raw_out,
            };
            cmd_stream::run(args).await?
        }
        Cmd::Events {
            usb: Some(usb),
            enable,
            count,
            access,
            ..
        } => {
            controlling_access(access, "events", true)?;
            cmd_usb::run_events(usb, &enable, count, json)?
        }
        Cmd::Events {
            ip,
            index,
            usb: None,
            iface: cmd_iface,
            port,
            enable,
            count,
            access,
        } => {
            let access = controlling_access(access, "events", false)?;
            let args = cmd_events::EventsArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                port,
                enable,
                count,
                access,
            };
            cmd_events::run(args, json).await?
        }
        Cmd::Chunks {
            usb: Some(usb),
            enable,
            selectors,
            access,
            ..
        } => {
            controlling_access(access, "chunks", true)?;
            cmd_usb::run_chunks(usb, enable, &selectors)?
        }
        Cmd::Chunks {
            ip,
            index,
            usb: None,
            enable,
            selectors,
            access,
        } => {
            let access = controlling_access(access, "chunks", false)?;
            cmd_chunks::run(ip, index, enable, selectors, iface, access, json).await?
        }
        Cmd::Bench {
            usb: Some(usb),
            duration_s,
            json_out,
            access,
            ..
        } => {
            common::chosen_access(access, viva_genicam::Access::Control, true)?;
            cmd_usb::run_bench(usb, duration_s, json, json_out.as_deref())?
        }
        Cmd::Bench {
            ip,
            index,
            usb: None,
            iface: cmd_iface,
            mode,
            group,
            port,
            packet_delay_ns,
            no_resend,
            duration_s,
            json_out,
            access,
            monitor,
        } => {
            let args = BenchArgs {
                access: stream_access(access, monitor),
                ip,
                index,
                iface: cmd_iface.or(iface),
                mode,
                group,
                port,
                packet_delay: packet_delay_ns.map(Duration::from_nanos),
                resend: !no_resend,
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
                assert_eq!(port, Some(12000));
                assert_eq!(group, Some("239.1.1.1".parse().unwrap()));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    /// No `--port` lets the operating system pick one (SR-03); resend is on
    /// unless asked off, and `GevSCPD` is left alone unless a delay is given.
    #[test]
    fn stream_defaults_leave_the_port_delay_and_resend_to_the_library() {
        let cli = Cli::parse_from(["viva-camctl", "stream"]);
        match cli.cmd {
            Cmd::Stream {
                port,
                packet_delay_ns,
                no_resend,
                ..
            } => assert_eq!((port, packet_delay_ns, no_resend), (None, None, false)),
            other => panic!("unexpected variant: {other:?}"),
        }
        let cli = Cli::parse_from([
            "viva-camctl",
            "bench",
            "--packet-delay-ns",
            "2000",
            "--no-resend",
        ]);
        match cli.cmd {
            Cmd::Bench {
                packet_delay_ns,
                no_resend,
                ..
            } => assert_eq!((packet_delay_ns, no_resend), (Some(2000), true)),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn stream_accepts_a_save_format() {
        for args in [
            &["viva-camctl", "stream", "--save-format", "png"][..],
            &["viva-camctl", "stream", "--save-format", "tiff"],
            &[
                "viva-camctl",
                "stream",
                "--usb",
                "0",
                "--save-format",
                "tiff",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?}");
        }
        // A USB3 Vision camera is named by --usb alone.
        assert!(
            Cli::try_parse_from(["viva-camctl", "stream", "--usb", "0", "--index", "1"]).is_err()
        );
    }

    /// API-11: events and chunks take --usb, as stream does.
    #[test]
    fn events_and_chunks_take_a_usb_camera() {
        for args in [
            &[
                "viva-camctl",
                "events",
                "--usb",
                "0",
                "--enable",
                "ExposureEnd",
            ][..],
            &["viva-camctl", "chunks", "--usb", "0", "--enable"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?}");
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
