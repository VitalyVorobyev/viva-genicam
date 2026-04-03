# genicam-rs

Pure Rust building blocks for **GenICam** with an **Ethernet-first (GigE Vision)** focus.  
Cargo workspace, modular crates (GenCP, GVCP/GVSP, GenApi core), and small examples.

## Current status (Apr 2026)
  * ✅ Discovery (GVCP) on selected NICs; enumerate devices. Loopback support for simulated cameras.
  * ✅ Control path (GVCP): read/write device memory; fetch GenICam XML. Correct GVCP wire format.
  * ✅ GenApi (Tier-1): NodeMap (Integer/Float/Enum/Bool/Command/Category), ranges, access modes.
  * ✅ GenApi (Tier-2): Converter, IntConverter, String, IntReg, MaskedIntReg nodes.
  * ✅ pValue delegation: Integer, Float, Enum, Boolean, Command nodes delegate to backing registers.
  * ✅ SwissKnife: full expression support (arithmetic, comparisons, ternary, logical, bitwise, math functions, Formula alias).
  * ✅ Selector-based address switching for common features (e.g., `GainSelector`).
  * ✅ High-level streaming API: `FrameStream` async iterator with auto-resend.
  * ✅ `connect_gige()` / `connect_gige_with_xml()` for camera connection with auto XML fetch.
  * ✅ Streaming (GVSP): packet reassembly, resend, MTU/packet size & delay, backpressure, stats.
  * ✅ Events & actions: message channel events; action commands (synchronization).
  * ✅ Time mapping & chunks: device↔host timestamp mapping; chunk data parsing.
  * ✅ **Sensor service** (`genicam-service`): Zenoh bridge for [genicam-studio](https://github.com/VitalyVorobyev/genicam-studio) — discovery, XML, node read/write, acquisition control, frame streaming.
  * ✅ Integration tests against `arv-fake-gv-camera` (aravis simulator).
  * ✅ macOS support: `Iface::from_system`, loopback discovery.
  * USB3 Vision transport (planned).

## Workspace layout

```
crates/
  genicp/            # GenCP encode/decode
  tl-gige/           # GigE Vision (GVCP/GVSP)
  genapi-xml/        # GenICam XML loader & schema-lite parser
  genapi-core/       # NodeMap & evaluation
  genicam/           # Public API facade
  genicam-service/   # Zenoh camera service for genicam-studio
  gencamctl/         # CLI binary
  pfnc/              # Pixel Format Naming Convention
  sfnc/              # Standard Feature Naming Convention
crates/genicam/examples/   # Small demos (see below)
crates/genicam/tests/      # Integration tests (arv-fake-gv-camera)
```

## Documentation

The main user & contributor documentation lives in the **mdBook** and the
generated **Rust API docs**.

- **Book (mdBook)** – sources are under [`book/`](book/).  
  Recommended starting points:
  - [`book/src/welcome.md`](book/src/welcome.md) – project overview.
  - [`book/src/tutorials/README.md`](book/src/tutorials/README.md) – step-by-step tutorials
    (discovery, registers, XML, streaming).

- **Rust API docs** – generated with:

  ```bash
  cargo doc --workspace --all-features

and served locally from target/doc, or published via GitHub Pages if you
enable that in CI.

For day-to-day usage, start with the Tutorials section of the book and only
dive into rustdoc when you need details of specific types and functions.

## Prereqs

  * Rust 1.75+ (pinned in `rust-toolchain.toml`)
  * Windows / Linux / macOS (tested on recent 64-bit versions; see docs for OS-specific notes)
  * Network:
    * Allow UDP broadcast on your capture NIC for discovery
    * Optional: enable jumbo frames if you plan to test high throughput

## Build & test

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Generate docs locally
cargo doc --workspace --no-deps
```

## Run examples

Examples live under the `genicam` crate. Run them via the facade crate target:

- **Discover devices (GVCP broadcast):**

```bash
cargo run -p genicam --example list_cameras
```

- **Fetch XML & print minimal metadata (control path):**

```bash
cargo run -p genicam --example get_set_feature
```

- **Grab frames (GVSP):**

```bash
cargo run -p genicam --example grab_gige
```

- **Events:**

```bash
cargo run -p genicam --example events_gige
```

- **Action command (broadcast):**

```bash
cargo run -p genicam --example action_trigger
```

- **Timestamp mapping:**

```bash
cargo run -p genicam --example time_sync
```

- **Selectors demo:**

```bash
cargo run -p genicam --example selectors_demo
```

See also: the [Tutorials](book/src/tutorials/README.md) section of the book
  for more complete, step-by-step guides.

## gencamctl CLI

The workspace now provides a `gencamctl` binary offering common camera control
operations from the command line. Enable more verbose logging with `-v` or
`RUST_LOG`, prefer JSON output with `--json`, and use `--iface <IPv4>` to select
the capture interface.

Examples:

```bash
# Discover GigE Vision cameras on the network
cargo run -p gencamctl -- list

# Inspect and configure GenApi features
cargo run -p gencamctl -- get --ip 192.168.0.10 --name ExposureTime
cargo run -p gencamctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000

# Receive a GVSP stream, auto-negotiate packet size, and save the first two frames
cargo run -p gencamctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2

# Configure and read GVCP events
cargo run -p gencamctl -- events --iface 192.168.0.5 --enable FrameStart,ExposureEnd --count 5

# Toggle chunk data features
cargo run -p gencamctl -- chunks --ip 192.168.0.10 --enable true --selectors Timestamp,ExposureTime

# Run a sustained streaming benchmark with a JSON report
cargo run -p gencamctl -- bench --ip 192.168.0.10 --duration-s 60 --json-out bench.json
```

For more examples and troubleshooting tips, see the
[Discovery](book/src/tutorials/discovery.md)
and [Streaming](book/src/tutorials/streaming.md) tutorials.

## genicam-service

The `genicam-service` binary bridges real GigE Vision cameras to
[genicam-studio](https://github.com/VitalyVorobyev/genicam-studio) via Zenoh.
It implements the Zenoh API contract defined in `genicam-studio/docs/zenoh-api.md`.

```bash
# Start the service (discovers cameras on the specified interface)
cargo run -p genicam-service -- --iface en0

# With verbose logging
cargo run -p genicam-service -- --iface en0 -vv
```

The service automatically discovers cameras, publishes device announcements,
serves GenICam XML, handles node read/write queries, and streams frames over
Zenoh when acquisition is started from genicam-studio.

## Integration testing

12 integration tests validate the full stack against `arv-fake-gv-camera-0.8`
from [Aravis](https://github.com/AravisProject/aravis) — covering discovery,
connection, XML parsing, feature read/write, command execution, and frame
streaming (all pass on macOS loopback).

```bash
# Install aravis (macOS)
brew install aravis

# Run integration tests (starts fake camera automatically)
cargo test -p genicam --test fake_camera -- --ignored --test-threads=1
```

## Troubleshooting

- No devices found: check NIC/interface selection and host firewall (UDP broadcast).
- Drops at high FPS: try jumbo frames, raise `SO_RCVBUF`, and enable packet delay.
- Windows: run as admin, allow UDP in firewall rules; jumbo frames must be enabled per NIC.

## License

MIT — see LICENSE.

## Acknowledgements

Standards: GenICam/GenApi (EMVA/A3), GigE Vision. Thanks to the open-source ecosystem for prior art and inspiration.
