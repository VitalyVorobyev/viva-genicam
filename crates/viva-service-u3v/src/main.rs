//! USB3 Vision Zenoh service — bridges USB3 Vision cameras to Viva Studio.
//!
//! The same service as `viva-service` (GigE Vision), on the same library:
//! it rescans the bus, serves every camera it finds, and publishes a camera
//! disconnected when it is gone (backlog SVC-04). `--fake` serves cameras in
//! process, for testing without USB hardware.

mod device;
mod discovery;
#[cfg(test)]
mod tests;

use std::time::Duration;

use clap::Parser;
use viva_genicam::pfnc::PixelFormat;
use viva_service::{init_tracing, run_service};

use crate::discovery::{FakeDiscovery, FakeSpec, UsbDiscovery};

#[derive(Parser, Debug)]
#[command(
    name = "viva-service-u3v",
    version,
    about = "USB3 Vision Zenoh service"
)]
struct Cli {
    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Serve in-process fake cameras instead of USB devices
    #[arg(long)]
    fake: bool,

    /// How many fake cameras to serve
    #[arg(long, default_value_t = 1)]
    fake_count: usize,

    /// Fake camera width
    #[arg(long, default_value_t = 640)]
    width: u32,

    /// Fake camera height
    #[arg(long, default_value_t = 480)]
    height: u32,

    /// Fake pixel format, by its PFNC name (Mono8, RGB8, Mono12Packed, ...)
    #[arg(long, default_value = "Mono8")]
    pixel_format: String,

    /// Seconds between rescans of the bus
    #[arg(long, default_value_t = 5)]
    discovery_interval_s: u64,

    /// Path to Zenoh configuration file
    #[arg(long)]
    zenoh_config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let interval = Duration::from_secs(cli.discovery_interval_s);
    let zenoh_config = cli.zenoh_config.as_deref();
    if !cli.fake {
        return run_service("viva-service-u3v", zenoh_config, UsbDiscovery, interval).await;
    }
    // Any PFNC name, and the lower-case spellings this flag took before.
    let name = match cli.pixel_format.as_str() {
        "mono8" => "Mono8",
        "rgb8" | "rgb8packed" => "RGB8",
        other => other,
    };
    let pixel_format = match PixelFormat::from_name(name) {
        PixelFormat::Unknown(_) => {
            return Err(format!("unknown pixel format {}", cli.pixel_format).into());
        }
        known => known.code(),
    };
    let discovery = FakeDiscovery::default();
    for index in 0..cli.fake_count {
        discovery.plug(FakeSpec {
            id: match index {
                0 => "cam-fake-u3v".to_string(),
                _ => format!("cam-fake-u3v-{index}"),
            },
            width: cli.width,
            height: cli.height,
            pixel_format,
        });
    }
    run_service("viva-service-u3v", zenoh_config, discovery, interval).await
}
