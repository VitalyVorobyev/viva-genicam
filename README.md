<p align="center">
  <img src="assets/viva-genicam-logo-and-text_opt.svg" alt="viva genicam" width="360">
</p>

<p align="center">
  Pure Rust building blocks for <b>GenICam</b>: <b>GigE Vision</b> and <b>USB3 Vision</b>.
</p>

<p align="center">
  <a href="https://github.com/VitalyVorobyev/viva-genicam/actions/workflows/ci.yml"><img src="https://github.com/VitalyVorobyev/viva-genicam/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/viva-genicam"><img src="https://img.shields.io/crates/v/viva-genicam.svg" alt="crates.io"></a>
  <a href="https://docs.rs/viva-genicam"><img src="https://docs.rs/viva-genicam/badge.svg" alt="docs.rs"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

> **Status -- pre-1.0, and working.** Users have run discovery, control and
> streaming against real GigE Vision cameras from several vendors, on Linux,
> Windows and macOS. The protocol layers are implemented against the EMVA
> specifications and covered by automated tests against in-process fake
> cameras, and by a corpus of dozens of real vendor GenApi XML descriptions.
>
> What pre-1.0 means in practice: the API still changes between releases, and
> **there is no camera in CI** -- hardware confirmations come from users with
> their own devices. So a camera nobody has tried yet may well hit something. If
> yours does, [open an issue](https://github.com/VitalyVorobyev/viva-genicam/issues/new/choose):
> that is how camera-specific bugs here get found and fixed.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

---

## Features

- **Discovery** -- find GigE Vision cameras via GVCP broadcast; USB3 Vision cameras via USB enumeration
- **Control** -- read and write device registers and GenApi features (Integer, Float, Enumeration, Boolean, Command, String, Register, SwissKnife, Converter, IntConverter)
- **Streaming** -- GigE: GVSP reassembly with backpressure, plus raw capture from event-vision cameras (packet resend is not wired into the receive path yet); U3V: async frame iterator over USB bulk reads
- **IP configuration** -- FORCEIP for temporary assignment; persistent IP registers for permanent configuration
- **Events & actions** -- subscribe to camera events; trigger synchronized acquisition via action commands
- **Time & chunks** -- map device timestamps to host time; parse chunk data (timestamp, exposure, gain)
- **Service bridge** -- expose cameras over [Zenoh](https://zenoh.io/) for Viva Studio (the desktop GUI in `studio/`)
- **No hardware required** -- built-in fake cameras (`viva-fake-gige`, `viva-fake-u3v`) for testing and demos

## Workspace layout

```
crates/
  viva-gencp/          GenCP encode/decode (transport-agnostic)
  viva-gige/           GigE Vision transport (GVCP/GVSP)
  viva-u3v/            USB3 Vision transport
  viva-genapi-xml/     GenICam XML parser
  viva-genapi/         GenApi node map & evaluation engine
  viva-genicam/        Public API facade (start here)
  viva-pfnc/           Pixel Format Naming Convention tables
  viva-sfnc/           Standard Feature Naming Convention constants
  viva-zenoh-api/      Message payloads and topic names for the services
  viva-service/        Zenoh bridge: GigE cameras -> Viva Studio
  viva-service-u3v/    Zenoh bridge: U3V cameras -> Viva Studio
  viva-camctl/         CLI binary
  viva-pygenicam/      Python bindings (PyO3) -- own workspace, own Cargo.lock
  viva-fake-gige/      Fake GigE camera for testing
  viva-fake-u3v/       Fake U3V camera for testing
```

## Viva Studio (GUI)

The desktop app for GenICam cameras lives in `studio/` as a separate Cargo
workspace in this repository. It is a React 19 + Tauri v2 application that
talks to `viva-service` / `viva-service-u3v` over Zenoh, so the published
library crates stay decoupled from GUI and Node toolchain concerns. See
`studio/CLAUDE.md` for build commands and invariants, and `docs/studio/`
for the Zenoh API contract and testing cookbook.

## Quick start

```bash
cargo add viva-genicam
```

```rust
use viva_genicam::gige;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Discover cameras on the network
    let devices = gige::discover(Duration::from_secs(1)).await?;
    println!("Found {} cameras", devices.len());

    // Connect to the first camera
    let mut camera = viva_genicam::connect_gige(&devices[0]).await?;

    // Read and write features
    let exposure = camera.get("ExposureTime")?;
    println!("ExposureTime = {exposure}");
    camera.set("ExposureTime", "5000")?;
    Ok(())
}
```

## Python

Pre-built wheels are published on PyPI — no C toolchain needed:

```bash
pip install viva-genicam
```

This also installs the [`viva-camctl`](#viva-camctl-cli) command, so the
diagnostics below are available without a Rust toolchain:

```bash
viva-camctl list
viva-camctl report --ip 192.168.0.10 --out viva-report.txt
```

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
cam = vg.connect_gige(cams[0])
cam.set_exposure_time_us(10_000.0)

with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()           # shape and dtype follow the pixel format
        break
```

See [`book/src/python.md`](book/src/python.md) for the full Python API.

## Documentation

- **[GenICam standards introduction](docs/standards.md)** -- what GenApi, GenCP, GVCP, SFNC, and PFNC are and how they map to crates
- **[Book (mdBook)](https://vitalyvorobyev.github.io/viva-genicam/)** -- tutorials, architecture, networking cookbook
- **[API reference (docs.rs)](https://docs.rs/viva-genicam)** -- generated Rust API docs
- **[Examples](crates/viva-genicam/examples/)** -- runnable examples covering discovery, streaming, events, chunks, and more. The book's Rust snippets are pulled from these files, so what you read there is code that compiles in CI
- **[System design](docs/design.md)** -- architecture, key abstractions, data flows, design tenets
- **[Decision records](docs/adrs/)** -- why the big choices were made
- **[Roadmap](docs/roadmap.md)** -- what's planned

## Prerequisites

- Rust 1.88+ (edition 2024)
- Windows / Linux / macOS
- Network: allow UDP broadcast on your capture NIC for discovery. Optional: jumbo frames for high throughput.

## Build & test

```bash
cargo build --workspace
cargo test --workspace
cargo doc --workspace --all-features --no-deps
```

## Run examples

```bash
# Discover cameras
cargo run -p viva-genicam --example list_cameras

# Read/write features
cargo run -p viva-genicam --example get_set_feature
cargo run -p viva-genicam --example get_set_feature -- --name Gain --value 3.0

# Fetch and inspect the camera's GenApi XML
cargo run -p viva-genicam --example fetch_xml

# Grab frames from the first camera found, on the host NIC you name
cargo run -p viva-genicam --example grab_gige -- --iface 192.168.0.5

# Zero-hardware demo (uses built-in fake camera)
cargo run -p viva-genicam --example demo_fake_camera
```

## viva-camctl CLI

The examples below use `cargo run` from a checkout. If you would rather not
build it, `pip install viva-genicam` installs the same CLI as a `viva-camctl`
command — the wheel links it in, so `viva-camctl report ...` works with no Rust
toolchain.

```bash
# Discover GigE Vision cameras
cargo run -p viva-camctl -- list

# Collect a diagnostic bundle to attach to a bug report
cargo run -p viva-camctl -- report --ip 192.168.0.10 --out viva-report.txt

# Dump the camera's GenApi XML (parses nothing, so it works on a camera we cannot open)
cargo run -p viva-camctl -- xml --ip 192.168.0.10 --out camera.xml

# Read a feature
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name ExposureTime

# Write a feature
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000

# Stream frames. By default the camera's GevSCPSPacketSize is kept.
# --auto: set from NIC MTU, then path-probe / bisect. --packet-size N: explicit ceiling.
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
# Cap for a known narrow path (e.g. a switch that only forwards ~9K frames):
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --packet-size 9000 --save 2

# Sustained streaming benchmark
cargo run -p viva-camctl -- bench --ip 192.168.0.10 --duration-s 60 --json-out bench.json

# Assign temporary IP via FORCEIP
cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --force

# Configure persistent IP
cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100

# Event-vision camera: append every EVT block to one file
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --save 0 --raw-out capture.evt3 --duration-s 10

# USB3 Vision: discover, read/write features, stream
cargo run -p viva-camctl -- list-usb
cargo run -p viva-camctl -- get-usb --name ExposureTime
cargo run -p viva-camctl -- set-usb --name ExposureTime --value 5000
cargo run -p viva-camctl -- stream-usb --save 3
```

## viva-service (Zenoh bridge)

```bash
# Start the GigE Vision service
cargo run -p viva-service -- --iface en0

# Start the USB3 Vision service (real camera)
cargo run -p viva-service-u3v

# Start the USB3 Vision service with a fake camera
cargo run -p viva-service-u3v -- --fake
```

The service discovers cameras, publishes device announcements, serves GenICam XML,
handles node read/write, and streams frames over Zenoh for Viva Studio
(the desktop GUI in `studio/`).

## Integration testing

Integration tests use the built-in `viva-fake-gige` camera simulator -- no
external tools or hardware required.

```bash
# All tests (unit + integration + service e2e)
cargo test --workspace

# Run a self-contained demo
cargo run -p viva-genicam --example demo_fake_camera
```

## Troubleshooting

- **No devices found** -- check NIC/interface selection and host firewall (UDP broadcast on port 3956)
- **Frame drops at high FPS** -- enable jumbo frames, raise `SO_RCVBUF`, enable inter-packet delay
- **Windows** -- allow the program through the firewall and check NIC power settings; see the book's [Networking guide](https://vitalyvorobyev.github.io/viva-genicam/networking.html#22-windows)

## License

MIT -- see [LICENSE](LICENSE).
