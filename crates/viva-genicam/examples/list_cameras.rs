use std::env;
use std::time::Duration;

use tracing::info;

fn parse_args() -> (Duration, Option<String>) {
    let mut args = env::args().skip(1);
    let mut timeout_ms: u64 = 500;
    let mut iface: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--timeout-ms" => {
                if let Some(value) = args.next() {
                    timeout_ms = value.parse().unwrap_or(timeout_ms);
                }
            }
            "--iface" => {
                iface = args.next();
            }
            _ => {}
        }
    }
    (Duration::from_millis(timeout_ms), iface)
}

fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let (timeout, iface) = parse_args();
    info!(?timeout, iface = iface.as_deref(), "starting discovery");
    let devices = if let Some(name) = iface.as_deref() {
        viva_genicam::gige::discover_on_interface(timeout, name).await?
    } else {
        viva_genicam::gige::discover(timeout).await?
    };

    if devices.is_empty() {
        println!("No cameras found.");
        return Ok(());
    }

    println!("{:<16} {:<17} {:<20} Model", "IP", "MAC", "Manufacturer");
    for dev in devices {
        println!(
            "{:<16} {:<17} {:<20} {}",
            dev.ip,
            format_mac(&dev.mac),
            dev.manufacturer.as_deref().unwrap_or("-"),
            dev.model.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}
