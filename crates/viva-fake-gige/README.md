# viva-fake-gige

In-process fake GigE Vision camera for testing and demos.

Speaks real GVCP and GVSP protocols over UDP on localhost, so integration tests can exercise the full camera stack without any hardware.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Real protocol** -- responds to GVCP discovery, GenCP register read/write, and GVSP streaming
- **Configurable** -- set resolution, frame rate, and pixel format via CLI flags
- **Self-contained** -- no USB devices, no network cameras, runs entirely on loopback
- **Integration tests** -- used by `cargo test --workspace` automatically

## Usage

```bash
# Start the fake camera (default: 640x480 @ 30 fps, Mono8)
cargo run -p viva-fake-gige

# Custom resolution and format
cargo run -p viva-fake-gige -- --width 1024 --height 768 --fps 15 --pixel-format rgb8
```

Then interact with it using `viva-camctl`:

```bash
cargo run -p viva-camctl -- list --iface 127.0.0.1
```

This crate is not published to crates.io.

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
