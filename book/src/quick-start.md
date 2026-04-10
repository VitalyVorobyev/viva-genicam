# Quick Start

This guide gets you from checkout to discovering cameras in minutes.

## Prerequisites
- **Rust**: MSRV 1.75+ (toolchain pinned via `rust-toolchain.toml`).
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
# List cameras via GVCP broadcast\ n cargo run -p viva-genicam --example list_cameras
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

# Fetch minimal XML metadata via control path (example)
cargo run -p viva-genicam --example get_set_feature
```

## Streaming (early GVSP)

```bash
# Receive a GVSP stream, auto‑negotiate packet size, save first two frames
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
```

## Windows specifics

* Run the terminal **as Administrator** the first time to let the firewall prompt appear.
* Add inbound **UDP rules** for discovery and streaming.
* Enable **jumbo frames** per NIC if your network supports it (helps at high FPS).

## Next steps

* Read the **Primer** for the concepts behind discovery, control, and streaming.
* Jump to the **Tutorial: Discover devices** for a step‑by‑step walkthrough with troubleshooting tips.
