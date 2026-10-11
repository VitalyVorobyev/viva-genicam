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

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

---

## Status and known gaps

Pre-1.0, and working: users have run discovery, control and streaming against
real GigE Vision cameras from several vendors, on Linux, Windows and macOS.
USB3 Vision has not run on a real camera yet, and the API still changes
between releases. What has been verified is on the user guide's
[welcome page](https://vitalyvorobyev.github.io/viva-genicam/welcome.html#status);
the full list of known gaps is [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191).
If your camera hits something, [open an issue](https://github.com/VitalyVorobyev/viva-genicam/issues/new/choose).

## Features

- **Discovery** -- find GigE Vision cameras via GVCP broadcast; USB3 Vision cameras via USB enumeration
- **Control** -- read and write device registers and GenApi features (Integer, Float, Enumeration, Boolean, Command, String, Register, SwissKnife, Converter, IntConverter)
- **Streaming** -- GigE: GVSP reassembly with backpressure, plus raw capture from event-vision cameras; U3V: async frame iterator over USB bulk reads
- **IP configuration** -- FORCEIP for temporary assignment; persistent IP registers for permanent configuration
- **Events & actions** -- subscribe to camera events; trigger synchronized acquisition via action commands
- **Time & chunks** -- map device timestamps to host time; parse chunk data (timestamp, exposure, gain)
- **Service bridge** -- expose cameras over [Zenoh](https://zenoh.io/) for Viva Studio (the desktop GUI in `studio/`)
- **No hardware required** -- built-in fake cameras (`viva-fake-gige`, `viva-fake-u3v`) for testing and demos

## Which part do I need?

- **A Rust application** -- `cargo add viva-genicam`, the facade over the whole stack ([Quick start](#quick-start)).
- **Python** -- `pip install viva-genicam` ([Python](#python)).
- **A command line** -- `viva-camctl`, from the release archives or with the Python package ([viva-camctl CLI](#viva-camctl-cli)).
- **A desktop app** -- [Viva Studio](#viva-studio), from the release downloads.
- **Cameras over the network** -- `viva-service` / `viva-service-u3v` serve them over Zenoh ([viva-service](#viva-service-zenoh-bridge)).

Every crate, and what it is for, is in the book's
[How the library fits together](https://vitalyvorobyev.github.io/viva-genicam/architecture.html).

## Viva Studio

A desktop app for GenICam cameras: find a camera, browse and change its
features, watch the live image, and save the diagnostic report we ask for. It
runs the library itself, so it is the quickest way to try viva-genicam on your
camera. Download it from the
[releases](https://github.com/VitalyVorobyev/viva-genicam/releases) — macOS
(Apple Silicon), Linux and Windows; the builds are unsigned, so the first
launch asks for confirmation. The book's
[Viva Studio](https://vitalyvorobyev.github.io/viva-genicam/studio.html) chapter
covers installing and using it.

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

The book's [Python chapter](https://vitalyvorobyev.github.io/viva-genicam/python.html) has the full Python API.

## Documentation

For users:

- **[User guide](https://vitalyvorobyev.github.io/viva-genicam/)** -- tutorials, networking, Python, Viva Studio, troubleshooting
- **[API reference (docs.rs)](https://docs.rs/viva-genicam)** -- the Rust API
- **[Examples](crates/viva-genicam/examples/)** -- runnable programs for discovery, streaming, events, chunks and more (`cargo run -p viva-genicam --example demo_fake_camera` needs no camera); the guide's Rust snippets come from them, so they compile in CI
- **[Reporting a camera we can't open](https://vitalyvorobyev.github.io/viva-genicam/reporting.html)** -- what to send us

For developers:

- **[CONTRIBUTING.md](CONTRIBUTING.md)** -- building, checks, tests, pull requests
- **[docs/](docs/README.md)** -- design, standards, decision records, roadmap and backlog

## viva-camctl CLI

`viva-camctl` discovers cameras, reads and writes features, streams and
benchmarks, sets IP addresses, and writes the diagnostic report a bug report
needs. It comes with `pip install viva-genicam`, in the release archives, and
from `cargo install viva-camctl`. Its commands and examples are in
[crates/viva-camctl/README.md](crates/viva-camctl/README.md).

## viva-service (Zenoh bridge)

`viva-service` (GigE Vision) and `viva-service-u3v` (USB3 Vision) serve cameras
over Zenoh — discovery, XML, feature read/write and frames — for Viva Studio's
remote mode and other clients: [crates/viva-service/README.md](crates/viva-service/README.md),
and the wire contract in [docs/studio/zenoh-api.md](docs/studio/zenoh-api.md).

## Troubleshooting

- **No devices found** -- check NIC/interface selection and host firewall (UDP broadcast on port 3956)
- **Frame drops at high FPS** -- enable jumbo frames, raise `SO_RCVBUF`, enable inter-packet delay
- **Windows** -- allow the program through the firewall and check NIC power settings; see the book's [Networking guide](https://vitalyvorobyev.github.io/viva-genicam/networking.html#22-windows)

## License

MIT -- see [LICENSE](LICENSE). The Python wheel also links libusb statically, which makes it
`MIT AND LGPL-2.1-or-later` ([details](crates/viva-pygenicam/README.md#licensing)).
