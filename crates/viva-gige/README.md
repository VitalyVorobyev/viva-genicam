# viva-gige

GigE Vision transport layer: GVCP discovery, GenCP-over-UDP register I/O, and GVSP streaming.

Implements the networking building blocks for communicating with GigE Vision cameras over Ethernet.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **GVCP discovery** -- broadcast and unicast device discovery on selected network interfaces
- **Register I/O** -- read/write device memory via GenCP-over-UDP with retry and backoff
- **GVSP streaming** -- packet parsing, frame reassembly, chunk parsing, backpressure. Resend building blocks (missing-packet tracking, the resend request) exist but are not wired into the receive path yet
- **Multicast** -- IGMP join/leave for multicast stream reception
- **Events** -- GVCP message channel for asynchronous camera events
- **Action commands** -- broadcast-triggered synchronized acquisition
- **Packet size** -- interface MTU lookup, GVSP test-packet requests and `GevSCPSPacketSize` read/write. The `viva-genicam` stream builder keeps the camera's packet size by default; sizing from the interface MTU is opt-in
- **macOS / Linux / Windows** -- cross-platform async UDP with `tokio`

## Usage

```bash
cargo add viva-gige
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

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
