# viva-service

Zenoh bridge that discovers GigE Vision cameras and exposes them as network-accessible services.

Client applications connect to the service for camera discovery, feature control, and image streaming — Viva Studio, the desktop app in [`studio/`](https://github.com/VitalyVorobyev/viva-genicam/tree/main/studio), is the reference consumer.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Auto-discovery** -- finds GigE Vision cameras on the specified network interface
- **GenICam XML** -- serves the device description XML via Zenoh queryable
- **Node read/write** -- live node value updates and feature control
- **Acquisition** -- start/stop image acquisition from client applications
- **Frame streaming** -- raw image data with a 16-byte binary header over Zenoh pub/sub; event-vision blocks on a topic of their own
- **Device lifecycle** -- connection/disconnection tracking with status announcements

## Usage

```bash
# Start the service
cargo run -p viva-service -- --iface en0

# With verbose logging
cargo run -p viva-service -- --iface en0 -vv
```

## Zenoh API

Cameras are exposed under `genicam/devices/{id}/`:

| Endpoint | Description |
|----------|-------------|
| `announce` | Periodic device announcements |
| `status` | Connection status of the device |
| `xml` | Queryable returning the GenICam XML |
| `nodes/{name}/value` | Node value updates |
| `nodes/{name}/state` | Queryable and updates carrying the full live `FeatureState` of one node |
| `nodes/{name}/set` | Queryable for writing node values |
| `nodes/{name}/execute` | Queryable for executing commands |
| `nodes/bulk/read` | Queryable for batch reads |
| `nodes/bulk/state` | Queryable returning the `FeatureState` of many nodes at once |
| `acquisition/control` | Start/stop acquisition |
| `acquisition/status` | Whether acquisition is running, with frame rate and drop count |
| `image` | Raw frame data with a 16-byte binary header |
| `image/meta` | Image geometry and pixel format |
| `evs` | Event-vision blocks (EVT 3.0 / 2.1) with a 24-byte binary header; never sent on `image` |

Wire types are defined in [`viva-zenoh-api`](https://crates.io/crates/viva-zenoh-api).

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-service)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
