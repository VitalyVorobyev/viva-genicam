# viva-zenoh-api

Shared wire protocol types for GenICam camera services over [Zenoh](https://zenoh.io/).

This crate defines the data contract between a camera service (`viva-service`) and its clients -- Viva Studio, the desktop app in [`studio/`](https://github.com/VitalyVorobyev/viva-genicam/tree/main/studio), is the reference consumer. It has **no Zenoh dependency** -- it is a pure data contract built on `serde`.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Versioning** -- `API_VERSION`, published by the service so a client can detect a contract mismatch
- **Discovery** -- `DeviceAnnounce`, `DeviceStatus`, `DeviceXmlResponse`
- **Node values** -- `NodeValueUpdate`, `NodeSetRequest`, `NodeOpResponse`, `BulkReadRequest` / `BulkReadResponse`
- **Live feature state** -- `FeatureState` (value, effective access mode, availability, numeric range, available enum entries), `NumericRange`, `CommandResult`
- **Acquisition** -- `AcquisitionControlRequest`, `AcquisitionCommand`, `AcquisitionStatus`
- **Image framing** -- `PixelFormat`, `ImageMeta`, `FrameHeader` (16-byte binary header on `image`)
- **Event-vision framing** -- `EvsHeader` (24-byte binary header) and `EvsFormat`, for blocks published on `keys::evs`
- **Key expressions** -- the `keys` module builds every topic path, e.g. `keys::node_value`, `keys::image`, `keys::evs`

## Usage

```bash
cargo add viva-zenoh-api
```

```rust
use viva_zenoh_api::keys;

let key = keys::node_value("camera-01", "ExposureTime");
assert_eq!(key, "genicam/devices/camera-01/nodes/ExposureTime/value");
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-zenoh-api)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
