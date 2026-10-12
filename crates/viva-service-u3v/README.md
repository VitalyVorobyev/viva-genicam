# viva-service-u3v

Zenoh bridge for USB3 Vision cameras.

Runs `viva-service`'s service — the same queries, streams and camera
lifecycle — over USB3 Vision: it rescans the bus, serves each camera it finds,
and publishes a camera that disappears as disconnected. `--fake` serves
in-process fake cameras instead, for testing without USB hardware.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Usage

```bash
# Start with a fake USB3 Vision camera (no hardware required)
cargo run -p viva-service-u3v -- --fake

# Custom resolution for the fake camera
cargo run -p viva-service-u3v -- --fake --width 1024 --height 768

# Two fake cameras in Mono12Packed (any PFNC name)
cargo run -p viva-service-u3v -- --fake --fake-count 2 --pixel-format Mono12Packed

# With verbose logging
cargo run -p viva-service-u3v -- --fake -vv
```

The service exposes U3V cameras over Zenoh using the same API as `viva-service`, so [Viva Studio](https://github.com/VitalyVorobyev/viva-genicam/tree/main/studio) works with both GigE and USB3 Vision cameras transparently.

This binary is not published to crates.io.

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
