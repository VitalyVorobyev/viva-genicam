# viva-zenoh-api

Shared Zenoh API payload types for GenICam camera services.

This crate defines the wire protocol types used between a GenICam camera service
and its clients (e.g. a Tauri desktop app). It has **no Zenoh dependency** — it
is a pure data contract built on `serde`.

## Usage

```toml
[dependencies]
viva-zenoh-api = "0.1"
```

```rust
use viva_zenoh_api::{DeviceAnnounce, NodeValueUpdate, keys};

// Build a Zenoh key expression for a node value
let key = keys::node_value("camera-01", "ExposureTime");
assert_eq!(key, "viva-genicam/devices/camera-01/nodes/ExposureTime/value");
```

## What's included

- **Discovery**: `DeviceAnnounce`, `DeviceStatus`
- **Node values**: `NodeValueUpdate`, `NodeSetRequest`, `NodeOpResponse`, bulk read types
- **Acquisition**: `AcquisitionCommand`, `AcquisitionStatus`
- **Image**: `PixelFormat`, `ImageMeta`, `FrameHeader` (16-byte binary header)
- **Key expressions**: Helper functions for building Zenoh topic paths

See the [workspace README](https://github.com/VitalyVorobyev/genicam-rs) for
the full project documentation.
