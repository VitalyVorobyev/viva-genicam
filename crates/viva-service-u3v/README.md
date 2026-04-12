# viva-service-u3v

Zenoh bridge for USB3 Vision cameras.

Extends the `viva-service` architecture to support USB3 Vision transport. Supports a `--fake` flag for testing without USB hardware.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Usage

```bash
# Start with a fake USB3 Vision camera (no hardware required)
cargo run -p viva-service-u3v -- --fake

# Custom resolution for the fake camera
cargo run -p viva-service-u3v -- --fake --width 1024 --height 768

# With verbose logging
cargo run -p viva-service-u3v -- --fake -vv
```

The service exposes U3V cameras over Zenoh using the same API as `viva-service`, so [genicam-studio](https://github.com/VitalyVorobyev/genicam-studio) works with both GigE and USB3 Vision cameras transparently.

This binary is not published to crates.io.

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
