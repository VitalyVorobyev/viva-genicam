//! GenICam camera service — bridges GigE Vision cameras to Zenoh for Viva
//! Studio.

use std::time::Duration;

use clap::Parser;
use viva_genicam::gige::IfaceSelector;
use viva_service::{GigeDiscovery, init_tracing, run_service};

#[derive(Parser, Debug)]
#[command(
    name = "viva-service",
    about = "GigE Vision camera service with a Zenoh API",
    version
)]
struct Cli {
    /// Host interface for camera discovery, named either by one of its IPv4
    /// addresses (`169.254.105.106`) or by its OS name (`en0`, `eth0`, or a
    /// GUID on Windows). `viva-camctl --iface` accepts the same two spellings.
    #[arg(long)]
    iface: Option<IfaceSelector>,

    /// Camera discovery timeout in milliseconds.
    #[arg(long, default_value_t = 2000)]
    discovery_timeout_ms: u64,

    /// Discovery poll interval in seconds.
    #[arg(long, default_value_t = 5)]
    discovery_interval_s: u64,

    /// Zenoh configuration file.
    #[arg(long)]
    zenoh_config: Option<String>,

    /// Log verbosity (-v, -vv, -vvv).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let discovery = GigeDiscovery::new(
        Duration::from_millis(cli.discovery_timeout_ms),
        cli.iface.clone(),
    );
    run_service(
        "viva-service",
        cli.zenoh_config.as_deref(),
        discovery,
        Duration::from_secs(cli.discovery_interval_s),
    )
    .await
}
