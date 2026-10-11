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
# Start the service (prebuilt in the release archives, or cargo run -p viva-service -- …)
viva-service --iface en0

# With verbose logging
viva-service --iface en0 -vv

# With a Zenoh configuration, as Viva Studio's remote mode needs
viva-service --iface en0 --zenoh-config zenoh-local.json5
```

Viva Studio's remote mode, including the configuration files both sides need,
is described in the user guide's
[Viva Studio → Remote mode](https://vitalyvorobyev.github.io/viva-genicam/studio.html#remote-mode).

## Zenoh API

Cameras are exposed under `genicam/devices/{id}/`. The keys, their payloads and
their semantics are specified in
[`docs/studio/zenoh-api.md`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/studio/zenoh-api.md);
the wire types are defined in [`viva-zenoh-api`](https://crates.io/crates/viva-zenoh-api).

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-service)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
