# viva-service

Zenoh bridge that discovers GigE Vision cameras and exposes them as network-accessible services.

Client applications like [genicam-studio](https://github.com/VitalyVorobyev/genicam-studio) connect to the service for camera discovery, feature control, and image streaming.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Auto-discovery** -- finds GigE Vision cameras on the specified network interface
- **GenICam XML** -- serves the device description XML via Zenoh queryable
- **Node read/write** -- live node value updates and feature control
- **Acquisition** -- start/stop image acquisition from client applications
- **Frame streaming** -- raw image data with 16-byte binary header over Zenoh pub/sub
- **Device lifecycle** -- connection/disconnection tracking with status announcements

## Usage

```bash
# Start the service
cargo run -p viva-service -- --iface en0

# With verbose logging
cargo run -p viva-service -- --iface en0 -vv
```

## Zenoh API

Cameras are exposed under `viva-genicam/devices/{id}/`:

| Endpoint | Description |
|----------|-------------|
| `announce` | Periodic device discovery announcements |
| `xml` | Queryable returning the GenICam XML |
| `nodes/{name}/value` | Live node value updates |
| `nodes/{name}/set` | Queryable for writing node values |
| `nodes/{name}/execute` | Queryable for executing commands |
| `nodes/bulk/read` | Queryable for batch reads |
| `acquisition/control` | Start/stop acquisition |
| `image` | Raw frame data with binary header |

Wire types are defined in [`viva-zenoh-api`](https://crates.io/crates/viva-zenoh-api).

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-service)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
