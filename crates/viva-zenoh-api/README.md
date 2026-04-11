# viva-zenoh-api

Shared wire protocol types for GenICam camera services over [Zenoh](https://zenoh.io/).

This crate defines the data contract between a camera service (`viva-service`) and its clients (e.g. [genicam-studio](https://github.com/VitalyVorobyev/genicam-studio)). It has **no Zenoh dependency** -- it is a pure data contract built on `serde`.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Discovery** -- `DeviceAnnounce`, `DeviceStatus`
- **Node values** -- `NodeValueUpdate`, `NodeSetRequest`, `NodeOpResponse`, bulk read types
- **Acquisition** -- `AcquisitionCommand`, `AcquisitionStatus`
- **Image framing** -- `PixelFormat`, `ImageMeta`, `FrameHeader` (16-byte binary header)
- **Key expressions** -- helper functions for building Zenoh topic paths

## Usage

```toml
[dependencies]
viva-zenoh-api = "0.1"
```

```rust
use viva_zenoh_api::{DeviceAnnounce, NodeValueUpdate, keys};

let key = keys::node_value("camera-01", "ExposureTime");
assert_eq!(key, "viva-genicam/devices/camera-01/nodes/ExposureTime/value");
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-zenoh-api)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
