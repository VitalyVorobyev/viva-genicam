# Quick Start

This guide gets you from checkout to discovering cameras in minutes.

## Prerequisites
- **Rust**: 1.88 or newer (edition 2024).
- **OS**: Windows, Linux, or macOS.
- **Network** (GigE Vision):
  - Allow **UDP broadcast** on the NIC you’ll use for discovery.
  - Optional: enable **jumbo frames** on that NIC for high‑throughput streaming tests.

## Build & Test
```bash
# From the repo root:
cargo build --workspace

# Run all tests
cargo test --workspace

# Generate local API docs (rustdoc)
cargo doc --workspace --no-deps
```

## First run: Discovery examples

You can try discovery in two ways—either via the high‑level `viva-genicam` crate example or the `viva-camctl` CLI.

### Option A: Example (genicam crate)

```bash
# List cameras via GVCP broadcast
cargo run -p viva-genicam --example list_cameras
```

### Option B: CLI (viva-camctl)

```bash
# Discover cameras on the selected interface (IPv4 of your NIC)
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

## Control path: read / write & XML

```bash
# Read a feature by name
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name ExposureTime

# Set a feature value
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000

# Dump the camera's GenApi XML
cargo run -p viva-camctl -- xml --ip 192.168.0.10 --out camera.xml
```

## When something does not work

```bash
# Collect everything a bug report needs, in one file
cargo run -p viva-camctl -- report --ip 192.168.0.10 --out viva-report.txt
```

The report lists the network interfaces the library can see, the camera's
reply to discovery, its bootstrap registers, its GenApi XML, and any feature
the camera has that this library could not build. Neither `report` nor `xml`
needs the camera to open successfully — that is what they are for — so both
still produce output when nothing else does. Attach the file to an
[issue](https://github.com/VitalyVorobyev/viva-genicam/issues/new/choose).

## Streaming

```bash
# Receive a GVSP stream and save 2 frames to the current directory.
# By default the camera's GevSCPSPacketSize is kept; --auto sizes packets from
# the NIC MTU, --packet-size N sets an explicit ceiling.
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
```

See [Streaming → Packet size and MTU](tutorials/streaming.md#41-packet-size-and-mtu).

## Windows specifics

The firewall prompt, NIC power saving and jumbo frames are the usual obstacles;
see [Networking → Windows](networking.md#22-windows).

## Next steps

* Read the [Primer](primer.md) for the concepts behind discovery, control, and streaming.
* Jump to the [Discovery tutorial](tutorials/discovery.md) for a step‑by‑step walkthrough with troubleshooting tips.
* No camera yet? [Testing without hardware](tutorials/fake-camera.md) runs everything against an in-process fake.
