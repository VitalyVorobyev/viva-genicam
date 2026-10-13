# viva-u3v

USB3 Vision transport layer for GenICam cameras.

Implements bootstrap register parsing, GenCP-over-USB control, device descriptor handling, bulk-endpoint streaming and the event interface. The `usb` feature enables real USB hardware access via [`nusb`](https://crates.io/crates/nusb), which is pure Rust: no libusb.

USB3 Vision support has run on one camera so far (a MATRIX VISION
mvBlueFOX3-2004G on macOS, over USB 2); known gaps are in
[issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191).

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Bootstrap registers** -- parse ABRM, SBRM, and SIRM register blocks
- **GenCP-over-USB** -- read/write device memory with proper framing
- **Device discovery** -- list USB3 Vision devices from the OS's device list, without opening them
- **Streaming** -- several bulk transfers in flight, leader/trailer parsing, chunk parsing, recovery from a failed transfer, a distinct error when the device is gone
- **Events** -- GenCP `EVENT_CMD`s from the event endpoint (EIRM)
- **Descriptors** -- parse USB3 Vision-specific interface descriptors
- **Mock transport** -- `UsbTransfer` trait enables testing without USB hardware

## Usage

```bash
cargo add viva-u3v --features usb
```

```rust
use viva_u3v::discover;

let devices = discover()?;
for dev in &devices {
    println!("{:04x}:{:04x}", dev.vendor_id, dev.product_id);
}
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `usb` | No | Enable real USB hardware access via `nusb` |

Without the `usb` feature, the crate provides protocol types and the `UsbTransfer` trait for use with mock or fake transports.

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-u3v)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
