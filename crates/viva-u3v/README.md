# viva-u3v

USB3 Vision transport layer for GenICam cameras.

Implements bootstrap register parsing, GenCP-over-USB control, device descriptor handling, and bulk-endpoint streaming. The `usb` feature enables real USB hardware access via [`rusb`](https://crates.io/crates/rusb).

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Bootstrap registers** -- parse ABRM, SBRM, and SIRM register blocks
- **GenCP-over-USB** -- read/write device memory with proper framing
- **Device discovery** -- enumerate USB3 Vision devices on the bus
- **Streaming** -- bulk-endpoint image transfer with leader/trailer parsing
- **Descriptors** -- parse USB3 Vision-specific interface descriptors
- **Mock transport** -- `UsbTransfer` trait enables testing without USB hardware

## Usage

```toml
[dependencies]
viva-u3v = { version = "0.1", features = ["usb"] }
```

```rust
use viva_u3v::discovery::discover;

let devices = discover()?;
for dev in &devices {
    println!("{:04x}:{:04x}", dev.vendor_id, dev.product_id);
}
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `usb` | No | Enable real USB hardware access via `rusb` |

Without the `usb` feature, the crate provides protocol types and the `UsbTransfer` trait for use with mock or fake transports.

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-u3v)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
