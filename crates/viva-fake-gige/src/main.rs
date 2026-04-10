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

    let camera = FakeCamera::builder()
        .width(args.width)
        .height(args.height)
        .fps(args.fps)
        .bind_ip(args.bind)
        .port(args.port)
        .build()
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to start fake camera: {e}");
            std::process::exit(1);
        });

    eprintln!(
        "Fake camera running on {}:{} ({}x{} Mono8 @ {} fps)",
        args.bind, args.port, args.width, args.height, args.fps,
    );
    eprintln!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for Ctrl+C");

    eprintln!("\nShutting down...");
    camera.stop();
}
