# viva-camctl

CLI tool for GenICam camera discovery, feature control, and streaming.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Install

```bash
cargo install viva-camctl
```

It also ships with the Python package: `pip install viva-genicam` installs a
`viva-camctl` command, linked into the extension module, so no Rust toolchain
is needed. From a checkout, `cargo run -p viva-camctl -- <command>` works too.

## Commands

| Command | Description |
|---------|-------------|
| `list` | Discover GigE Vision cameras on the network |
| `get` | Read a GenApi feature value |
| `set` | Write a GenApi feature value |
| `execute` | Execute a GenApi Command feature (e.g. `UserSetLoad`) |
| `xml` | Dump the camera's GenApi XML without parsing it |
| `report` | Collect a diagnostic bundle to attach to a bug report |
| `stream` | Start a GVSP stream; save frames, or raw event-vision blocks |
| `events` | Configure and read GVCP events |
| `chunks` | Toggle chunk mode and chunk selectors |
| `bench` | Sustained streaming benchmark with optional JSON report |
| `set-ip` | Assign a camera's IP address by MAC (FORCEIP with `--force`, persistent otherwise) |
| `list-usb` | Discover USB3 Vision cameras |
| `get-usb` | Read a feature from a USB3 Vision camera |
| `set-usb` | Write a feature to a USB3 Vision camera |
| `stream-usb` | Stream frames from a USB3 Vision camera |

`viva-camctl <command> --help` lists each command's flags.

## Examples

```bash
# Discover cameras
viva-camctl list

# Read and write a feature
viva-camctl get --ip 192.168.0.10 --name ExposureTime
viva-camctl set --ip 192.168.0.10 --name ExposureTime --value 5000

# Everything a bug report needs; works on a camera the library cannot open
viva-camctl report --ip 192.168.0.10 --out viva-report.txt
viva-camctl xml --ip 192.168.0.10 --out camera.xml

# Stream and save 2 frames. By default the camera's GevSCPSPacketSize is kept;
# --auto sizes packets from the NIC MTU, --packet-size N sets an explicit ceiling.
viva-camctl stream --ip 192.168.0.10 --iface 192.168.0.5 --save 2
viva-camctl stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
viva-camctl stream --ip 192.168.0.10 --iface 192.168.0.5 --packet-size 9000 --save 2

# Event-vision camera: append every EVT block to one file
viva-camctl stream --ip 192.168.0.10 --save 0 --raw-out capture.evt3 --duration-s 10

# Run a 60-second streaming benchmark
viva-camctl bench --ip 192.168.0.10 --duration-s 60 --json-out bench.json
```

## Global options

These go **before** the command name:

- `-v` / `-vv` -- log at `debug` / `trace` (`RUST_LOG` overrides)
- `--json` -- machine-readable output where the command supports it
- `--iface <IFACE>` -- the host interface to use, named by one of its IPv4
  addresses (`192.168.0.5`) or its OS name (`eth0`, or a GUID on Windows)

```bash
viva-camctl --json --iface 192.168.0.5 get --ip 192.168.0.10 --name Width
```

`list`, `xml`, `report`, `stream`, `events` and `bench` also accept `--iface`
after the command name; the other commands take it only in the global
position.

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
