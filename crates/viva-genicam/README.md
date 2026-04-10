# viva-genicam

High-level GenICam facade: camera discovery, feature control, image streaming, events, and action commands.

This is the main entry point for the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace. It re-exports the lower-level crates and provides convenience wrappers so you can get started with a single dependency.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Discovery** -- find GigE Vision cameras on any network interface
- **Connect & control** -- `connect_gige()` one-liner for camera connection with automatic XML fetch
- **Feature access** -- typed get/set for Integer, Float, Enum, Boolean, Command, String features
- **Streaming** -- `FrameStream` async iterator with resend, reassembly, and backpressure
- **Events & actions** -- subscribe to camera events; trigger synchronized acquisition
- **Chunks & timestamps** -- parse chunk data; map device timestamps to host time
- **USB3 Vision** -- optional `u3v` feature for USB3 Vision cameras

## Usage

```toml
[dependencies]
viva-genicam = "0.1"
```

```rust
use viva_genicam::{gige, Camera};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let devices = gige::discover(Duration::from_secs(1)).await?;
    let (mut camera, _xml) = viva_genicam::connect_gige(&devices[0]).await?;
    camera.set("ExposureTime", "5000")?;
    let val = camera.get("ExposureTime")?;
    println!("ExposureTime = {val}");
    Ok(())
}
```

## Feature flags

| Flag | Description |
|------|-------------|
| `u3v` | Enable USB3 Vision transport |
| `u3v-usb` | Enable USB3 Vision with real USB hardware access (includes `u3v`) |

## Documentation

- [API reference (docs.rs)](https://docs.rs/viva-genicam)
- [Book & tutorials](https://vitalyvorobyev.github.io/genicam-rs/)
- [Examples](https://github.com/VitalyVorobyev/genicam-rs/tree/main/crates/viva-genicam/examples)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
