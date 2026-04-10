# viva-service

Zenoh bridge exposing GenICam cameras as network-accessible services.

This crate provides a binary (`viva-service`) that discovers GigE Vision
cameras on the local network and makes them available over
[Zenoh](https://zenoh.io/) pub/sub and queryables. Client applications (like
[genicam-studio](https://github.com/VitalyVorobyev/genicam-studio)) connect
to the service for camera discovery, feature control, and image streaming.

## Usage

```bash
cargo run -p viva-service -- --iface en0
```

## Zenoh API

The service exposes cameras under `viva-genicam/devices/{id}/`:

- `announce` -- periodic device discovery announcements
- `xml` -- queryable returning the GenICam XML
- `nodes/{name}/value` -- live node value updates
- `nodes/{name}/set` -- queryable for writing node values
- `nodes/{name}/execute` -- queryable for executing commands
- `nodes/bulk/read` -- queryable for batch reads
- `acquisition/control` -- start/stop acquisition
- `image` -- raw frame data with 16-byte binary header

Wire types are defined in the
[`viva-zenoh-api`](https://crates.io/crates/viva-zenoh-api) crate.

See the [workspace README](https://github.com/VitalyVorobyev/genicam-rs)
for the full project documentation.
