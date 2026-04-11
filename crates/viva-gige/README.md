# viva-gige

GigE Vision transport layer: GVCP discovery, GenCP-over-UDP register I/O, and GVSP streaming.

Implements the networking building blocks for communicating with GigE Vision cameras over Ethernet.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **GVCP discovery** -- broadcast and unicast device discovery on selected network interfaces
- **Register I/O** -- read/write device memory via GenCP-over-UDP with retry and backoff
- **GVSP streaming** -- frame reassembly, packet resend with bitmap tracking, backpressure
- **Multicast** -- IGMP join/leave for multicast stream reception
- **Events** -- GVCP message channel for asynchronous camera events
- **Action commands** -- broadcast-triggered synchronized acquisition
- **MTU negotiation** -- automatic packet size detection from interface MTU
- **macOS / Linux / Windows** -- cross-platform async UDP with `tokio`

## Usage

```toml
[dependencies]
viva-gige = "0.1"
```

```rust
use viva_gige::discover;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let devices = discover(Duration::from_secs(1)).await?;
    for dev in &devices {
        println!("{dev:?}");
    }
    Ok(())
}
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-gige)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
