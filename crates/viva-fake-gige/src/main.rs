//! Standalone fake GigE Vision camera server.
//!
//! Starts a simulated camera on localhost and keeps it running until Ctrl+C.
//! Useful for manual testing with `viva-service` and `genicam-studio`.
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

    let camera = FakeCamera::builder()
        .width(args.width)
        .height(args.height)
        .fps(args.fps)
        .pixel_format(pfnc_code)
        .bind_ip(args.bind)
        .port(args.port)
        .build()
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to start fake camera: {e}");
            std::process::exit(1);
        });

    eprintln!(
        "Fake camera running on {}:{} ({}x{} {} @ {} fps)",
        args.bind, args.port, args.width, args.height, pf_name, args.fps,
    );
    eprintln!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");

    eprintln!("\nShutting down...");
    camera.stop();
}
