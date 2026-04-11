use std::net::Ipv4Addr;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{ArgAction, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use viva_camctl::cmd_bench::{self, BenchArgs};
use viva_camctl::cmd_chunks;
use viva_camctl::cmd_events;
use viva_camctl::cmd_get;
use viva_camctl::cmd_list;
use viva_camctl::cmd_set;
use viva_camctl::cmd_stream::{self, StreamArgs};
use viva_camctl::cmd_usb;

#[derive(Parser, Debug)]
#[command(name = "viva-camctl", version, about = "GenICam CLI")]
struct Cli {
    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,
    /// Output JSON where applicable
    #[arg(long)]
    json: bool,
    /// Preferred interface IPv4 address
    #[arg(long)]
    iface: Option<Ipv4Addr>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Discover cameras (GVCP)
    List {
        #[arg(long, default_value_t = 1000)]
        timeout_ms: u64,
        #[arg(long)]
        iface: Option<Ipv4Addr>,
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
    /// Start GVSP stream (uni-/multicast)
    Stream {
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long)]
        iface: Option<Ipv4Addr>,
        #[arg(long, default_value = "unicast")]
        mode: String,
        #[arg(long)]
        group: Option<Ipv4Addr>,
        #[arg(long, default_value_t = 10040)]
        port: u16,
        #[arg(long)]
        auto: bool,
        #[arg(long, default_value_t = 1)]
        save: usize,
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
        #[arg(long)]
        iface: Option<Ipv4Addr>,
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
        #[arg(long)]
        iface: Option<Ipv4Addr>,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli {
        verbose,
        json,
        iface,
        cmd,
    } = Cli::parse();

    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| level.into()),
        ))
        .with_target(false)
        .init();

    match cmd {
        Cmd::List {
            timeout_ms,
            iface: cmd_iface,
        } => {
            let iface = cmd_iface.or(iface);
            cmd_list::run(timeout_ms, iface, json).await?
        }
        Cmd::Get { ip, index, name } => cmd_get::run(ip, index, name, iface, json).await?,
        Cmd::Set {
            ip,
            index,
            name,
            value,
        } => cmd_set::run(ip, index, name, value, iface, json).await?,
        Cmd::Stream {
            ip,
            index,
            iface: cmd_iface,
            mode,
            group,
            port,
            auto,
            save,
            rgb,
            duration_s,
        } => {
            let args = StreamArgs {
                ip,
                index,
                iface: cmd_iface.or(iface),
                mode,
                group,
                port,
                auto,
                save,
                rgb,
                duration_s,
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
        } => {
            let iface = cmd_iface.or(iface).ok_or_else(|| {
                anyhow!("events require --iface or a global --iface IPv4 address")
            })?;
            cmd_events::run(ip, index, iface, port, enable, count, json).await?
        }
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
        Cmd::ListUsb => cmd_usb::run_list(json)?,
        Cmd::GetUsb { index, name } => cmd_usb::run_get(index, name, json)?,
        Cmd::SetUsb { index, name, value } => cmd_usb::run_set(index, name, value, json)?,
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
