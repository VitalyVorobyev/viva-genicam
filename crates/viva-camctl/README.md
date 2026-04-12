# viva-camctl

CLI tool for GenICam camera discovery, feature control, and streaming.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Install

This binary is not published to crates.io. Build from source:

```bash
cargo install --path crates/viva-camctl
```

## Commands

| Command | Description |
|---------|-------------|
| `list` | Discover GigE Vision cameras on the network |
| `list-usb` | Discover USB3 Vision cameras |
| `get` | Read a GenApi feature value |
| `set` | Write a GenApi feature value |
| `stream` | Start a GVSP stream and save frames |
| `events` | Configure and read GVCP events |
| `chunks` | Toggle chunk data features |
| `bench` | Sustained streaming benchmark with JSON report |

## Examples

```bash
# Discover cameras
viva-camctl list

# Read a feature
viva-camctl get --ip 192.168.0.10 --name ExposureTime

# Write a feature
viva-camctl set --ip 192.168.0.10 --name ExposureTime --value 5000

# Stream with auto packet-size negotiation, save 2 frames
viva-camctl stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2

# Run a 60-second streaming benchmark
viva-camctl bench --ip 192.168.0.10 --duration-s 60 --json-out bench.json
```

## Options

- `-v` / `-vv` -- increase log verbosity
- `--json` -- output in JSON format
- `--iface <IPv4>` -- preferred network interface

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
