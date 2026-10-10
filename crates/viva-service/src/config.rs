use clap::Parser;
use std::time::Duration;
use viva_genicam::gige::nic::IfaceSelector;

#[derive(Parser, Debug)]
#[command(
    name = "viva-service",
    about = "GigE Vision camera service with a Zenoh API",
    version
)]
pub struct Cli {
    /// Host interface for camera discovery, named either by one of its IPv4
    /// addresses (`169.254.105.106`) or by its OS name (`en0`, `eth0`, or a
    /// GUID on Windows). `viva-camctl --iface` accepts the same two spellings.
    #[arg(long)]
    pub iface: Option<IfaceSelector>,

    /// Camera discovery timeout in milliseconds.
    #[arg(long, default_value_t = 2000)]
    pub discovery_timeout_ms: u64,

    /// Discovery poll interval in seconds.
    #[arg(long, default_value_t = 5)]
    pub discovery_interval_s: u64,

    /// Zenoh configuration file.
    #[arg(long)]
    pub zenoh_config: Option<String>,

    /// Log verbosity (-v, -vv, -vvv).
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Cli {
    pub fn discovery_timeout(&self) -> Duration {
        Duration::from_millis(self.discovery_timeout_ms)
    }

    pub fn discovery_interval(&self) -> Duration {
        Duration::from_secs(self.discovery_interval_s)
    }
}
