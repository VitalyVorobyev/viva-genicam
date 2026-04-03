use clap::Parser;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "genicam-service",
    about = "GenICam camera service with Zenoh API"
)]
pub struct Cli {
    /// Network interface for camera discovery (e.g. "en0", "eth0").
    #[arg(long)]
    pub iface: Option<String>,

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
