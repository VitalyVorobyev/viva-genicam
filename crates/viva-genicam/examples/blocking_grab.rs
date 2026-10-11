//! Grab frames from a GigE Vision camera in a program with no async runtime.
//!
//! ```bash
//! cargo run -p viva-genicam --example blocking_grab              # the fake camera
//! cargo run -p viva-genicam --example blocking_grab -- 192.168.1.20
//! ```
//!
//! Without an address it starts the in-process fake camera, which needs a
//! runtime of its own to answer; the program itself never touches one.

use std::net::Ipv4Addr;
use std::time::Duration;

use viva_genicam::gige::DiscoveryOptions;
use viva_genicam::{StreamOptions, blocking};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let address: Option<Ipv4Addr> = std::env::args().nth(1).map(|ip| ip.parse()).transpose()?;
    // The fake camera, when no address is given: it runs on a runtime of its
    // own, as a real camera runs on its own hardware.
    let fake_runtime = tokio::runtime::Runtime::new()?;
    let _fake = match address {
        Some(_) => None,
        None => Some(
            fake_runtime.block_on(
                viva_fake_gige::FakeCamera::builder()
                    .bind_ip(Ipv4Addr::LOCALHOST)
                    .port(3956)
                    .build(),
            )?,
        ),
    };

    // ANCHOR: discover
    // Each call blocks for its timeout.
    let timeout = Duration::from_millis(500);
    let device = match address {
        Some(ip) => blocking::discover_ip(ip, timeout)?.ok_or("that camera did not answer")?,
        // Loopback included, for the fake camera.
        None => blocking::discover_with(timeout, &DiscoveryOptions::new().include_loopback(true))?
            .into_iter()
            .find(|device| device.ip.is_loopback())
            .ok_or("no camera found")?,
    };
    // ANCHOR_END: discover

    // ANCHOR: open
    // The camera carries a small runtime of its own, shut down on drop.
    let mut camera = blocking::connect(&device)?;
    // Executes UserSetLoad and waits for the camera to finish; any command
    // waits the same way with `execute_and_wait_blocking`.
    camera.load_user_set("Default")?;
    camera.set_exposure_time(5_000.0)?;
    // ANCHOR_END: open

    // ANCHOR: grab
    let mut frames = camera.open_stream(StreamOptions::new())?;
    camera.acquisition_start()?;
    for _ in 0..5 {
        match frames.next_frame_blocking(Duration::from_secs(2))? {
            Some(frame) => println!(
                "frame {}x{} {:?}, {} bytes",
                frame.width,
                frame.height,
                frame.pixel_format,
                frame.payload.len()
            ),
            None => break,
        }
    }
    camera.acquisition_stop()?;
    // ANCHOR_END: grab
    Ok(())
}
