//! Standalone fake GigE Vision camera server.
//!
//! Starts a simulated camera on localhost and keeps it running until Ctrl+C.
//! Useful for manual testing with `viva-service` and Viva Studio.
//!
//! ```bash
//! # Start with defaults (640x480 Mono8, 30 fps, port 3956)
//! cargo run -p viva-fake-gige
//!
//! # Custom configuration
//! cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 10
//!
//! # Start in RGB8 mode
//! cargo run -p viva-fake-gige -- --pixel-format rgb8
//!
//! # A second camera of the same model, told apart by MAC, serial and name.
//! # It needs its own address: two fakes can share 127.0.0.1:3956, but a
//! # unicast discovery reaches only one of them. 127.0.0.2 must first exist on
//! # the loopback interface (macOS: `sudo ifconfig lo0 alias 127.0.0.2 up`;
//! # Linux: `sudo ip addr add 127.0.0.2/8 dev lo`).
//! cargo run -p viva-fake-gige -- --bind 127.0.0.2 --mac 02:00:00:00:00:02 \
//!     --serial FAKE-002 --user-name Right
//! ```

use std::net::Ipv4Addr;

use clap::Parser;
use viva_fake_gige::FakeCamera;

#[derive(Parser)]
#[command(name = "viva-fake-gige", about = "Fake GigE Vision camera for testing")]
struct Args {
    /// Image width in pixels.
    #[arg(long, default_value_t = 640)]
    width: u32,

    /// Image height in pixels.
    #[arg(long, default_value_t = 480)]
    height: u32,

    /// Target frame rate (frames per second).
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// Pixel format: mono8 or rgb8.
    #[arg(long, default_value = "mono8")]
    pixel_format: String,

    /// IPv4 address to bind the GVCP socket to.
    #[arg(long, default_value = "127.0.0.1")]
    bind: Ipv4Addr,

    /// GVCP control port.
    #[arg(long, default_value_t = 3956)]
    port: u16,

    /// Release control privilege when the controller stops sending GVCP
    /// commands for longer than GevHeartbeatTimeout, as a real device does.
    #[arg(long)]
    enforce_heartbeat: bool,

    /// Silently clamp GevSCPSPacketSize to this many bytes, as a real camera
    /// caps it. Use 1500 to reproduce the jumbo-link failure in #112.
    #[arg(long)]
    max_packet_size: Option<u32>,

    /// MAC address to report, as AA:BB:CC:DD:EE:FF. Services key a camera by
    /// its MAC, so a second fake needs its own [default: DE:AD:BE:EF:CA:FE]
    #[arg(long, value_parser = parse_mac)]
    mac: Option<[u8; 6]>,

    /// Model name to report (at most 32 bytes on the wire).
    #[arg(long, default_value = viva_fake_gige::FAKE_MODEL)]
    model: String,

    /// Serial number to report (at most 16 bytes on the wire).
    #[arg(long, default_value = viva_fake_gige::FAKE_SERIAL)]
    serial: String,

    /// User-defined name to report (at most 16 bytes on the wire). Pass an
    /// empty string to report none.
    #[arg(long, default_value = viva_fake_gige::FAKE_USER_NAME)]
    user_name: String,
}

/// Parse `AA:BB:CC:DD:EE:FF` (either case, `:` or `-` separated).
fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(format!("expected six hex bytes, got '{s}'"));
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        *byte = u8::from_str_radix(part, 16).map_err(|_| format!("invalid hex byte '{part}'"))?;
    }
    Ok(mac)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let pfnc_code = match args.pixel_format.to_ascii_lowercase().as_str() {
        "mono8" => viva_fake_gige::MONO8,
        "rgb8" => viva_fake_gige::RGB8,
        other => {
            eprintln!("Unknown pixel format '{other}'. Supported: mono8, rgb8");
            std::process::exit(1);
        }
    };

    let pf_name = args.pixel_format.to_ascii_uppercase();

    let mut builder = FakeCamera::builder()
        .width(args.width)
        .height(args.height)
        .fps(args.fps)
        .pixel_format(pfnc_code)
        .bind_ip(args.bind)
        .port(args.port)
        .enforce_heartbeat(args.enforce_heartbeat)
        .model(args.model.as_str())
        .serial(args.serial.as_str())
        .user_name(args.user_name.as_str());
    if let Some(mac) = args.mac {
        builder = builder.mac(mac);
    }
    if let Some(max) = args.max_packet_size {
        builder = builder.max_packet_size(max);
    }

    let camera = builder.build().await.unwrap_or_else(|e| {
        eprintln!("Failed to start fake camera: {e}");
        std::process::exit(1);
    });

    eprintln!(
        "Fake camera running on {}:{} ({}x{} {} @ {} fps)",
        args.bind, args.port, args.width, args.height, pf_name, args.fps,
    );
    eprintln!(
        "Identity: model '{}', serial '{}', user name '{}'",
        args.model, args.serial, args.user_name,
    );
    eprintln!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");

    eprintln!("\nShutting down...");
    camera.stop().await;
}

#[cfg(test)]
mod tests {
    use super::parse_mac;

    #[test]
    fn parse_mac_accepts_either_case_and_separator() {
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        assert_eq!(parse_mac("DE:AD:BE:EF:CA:FE"), Ok(mac));
        assert_eq!(parse_mac("de-ad-be-ef-ca-fe"), Ok(mac));
    }

    #[test]
    fn parse_mac_rejects_malformed_input() {
        assert!(parse_mac("DE:AD:BE:EF:CA").is_err());
        assert!(parse_mac("DE:AD:BE:EF:CA:ZZ").is_err());
        assert!(parse_mac("DE:AD:BE:EF:CA:FE:00").is_err());
    }
}
