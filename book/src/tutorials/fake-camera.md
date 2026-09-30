# Testing without hardware

This tutorial shows how to evaluate the full viva-genicam stack without
physical cameras or external tools. The `viva-fake-gige` crate provides an
in-process GigE Vision camera simulator that speaks real GVCP/GVSP protocols
on localhost.

## Quick start

```bash
# Run the self-contained demo
cargo run -p viva-genicam --example demo_fake_camera
```

The demo starts a camera, discovers it, connects, reads and writes features, and
streams five frames — the whole stack, on loopback:

```
Starting fake GigE Vision camera on 127.0.0.1:3956 ...
Discovering cameras (2 s timeout) ...
  Found 1 device(s):
    IP: 127.0.0.1  Model: FakeGigE  Manufacturer: viva-genicam

Connecting to 127.0.0.1 ...
  Connected. GenApi XML: … bytes, … features.

Reading camera features:
  Width = 640
  ...

Setting Width = 320, ExposureTime = 10000 ...
  Width readback = 320

Streaming 5 frames ...
  Frame 1: 320x480 Mono8 payload=153600B ts=7393542
  ...

Demo complete. All operations succeeded without hardware.
```

## What the fake camera supports

| Feature | Status |
|---------|--------|
| GVCP discovery (broadcast on loopback) | Supported |
| GenCP register read/write (READREG, WRITEREG, READMEM, WRITEMEM) | Supported |
| Control Channel Privilege (CCP) | Supported |
| GenApi XML with SFNC features | Width, Height, PixelFormat, ExposureTime, Gain |
| GVSP frame streaming | Synthetic gradient images at configurable FPS |
| Device timestamps (1 GHz tick rate) | Supported (ns since acquisition start) |
| Timestamp latch (GevTimestampValue) | Supported |
| Chunk data (Timestamp, ExposureTime) | Supported when ChunkModeActive=1 |

## Running integration tests

The workspace's integration tests use the fake cameras automatically, so
`cargo test --workspace` needs no hardware. The individual suites are listed in
[Running the tests](../testing.md).

## Using the fake camera in your own code

Add `viva-fake-gige` as a dev-dependency:

```toml
[dev-dependencies]
viva-fake-gige = { git = "https://github.com/VitalyVorobyev/viva-genicam" }
```

Start a fake camera in your test. The value `build()` returns is a guard: the
camera answers GVCP and streams GVSP for as long as it is alive, and shuts down
when dropped.

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:fake_camera}}
```

Then discover it — note `discover_all`, not `discover`, because the fake lives
on loopback:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:connect}}
```

From there it is the ordinary API:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:read_features}}
```

The full program those excerpts come from is
[`examples/demo_fake_camera.rs`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/crates/viva-genicam/examples/demo_fake_camera.rs),
which goes on to build a `FrameStream` and grab five frames.

## Running as a standalone server

The `viva-fake-gige` binary starts a long-running fake camera that stays alive
until Ctrl+C. This is the way to test interactively with `viva-camctl`, or with
`viva-service` and Viva Studio.

```bash
# Terminal 1: start the fake camera
cargo run -p viva-fake-gige

# Custom dimensions, frame rate and pixel format (mono8 or rgb8)
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15 --pixel-format rgb8
```

Output:
```
Fake camera running on 127.0.0.1:3956 (640x480 MONO8 @ 30 fps)
Identity: model '…', serial '…', user name '…'
Press Ctrl+C to stop.
```

`cargo run -p viva-fake-gige -- --help` lists the remaining options.

## Using the CLI with the fake camera

With the fake camera running in Terminal 1, use `viva-camctl` in Terminal 2:

```bash
# Discover (use --iface to include loopback)
cargo run -p viva-camctl -- list --iface 127.0.0.1

# Read / write features
cargo run -p viva-camctl -- get --ip 127.0.0.1 --name Width
cargo run -p viva-camctl -- set --ip 127.0.0.1 --name Width --value 512
cargo run -p viva-camctl -- get --ip 127.0.0.1 --name DeviceModelName
```

## E2E testing with Viva Studio

Viva Studio lives in `studio/` in this repository, as a separate Cargo
workspace. The full-stack test uses three terminals:

```bash
# Terminal 1: fake camera
cargo run -p viva-fake-gige

# Terminal 2: camera service (bridges the camera onto Zenoh).
# --zenoh-config is required on the service side so Studio can connect via TCP.
cargo run -p viva-service -- \
  --iface lo0 \
  --zenoh-config studio/config/zenoh-local.json5
# On Linux: --iface lo

# Terminal 3: the desktop app. ZENOH_CONFIG is what selects remote mode, and it
# has to be absolute — the `cd` below would break a relative path.
export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri
cargo tauri dev
```

Studio should discover the fake camera, show its feature tree, and stream
gradient images in the viewer. The chip in the header reads **Remote**.

If you skip `ZENOH_CONFIG`, the app still starts — in *embedded* mode, where it
drives cameras directly instead of through the service. That mode does not scan
loopback, so this fake camera cannot appear and the device list simply stays
empty. The header chip reads **Embedded**, which is how you tell the two apart.

For USB3 Vision the service can host its own fake, so two terminals suffice:

```bash
cargo run -p viva-service-u3v -- --fake --zenoh-config studio/config/zenoh-local.json5

export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri && cargo tauri dev
```

## Fake camera configuration

The `FakeCameraBuilder` supports:

```rust,ignore
let camera = FakeCamera::builder()
    .width(1920)                     // default: 640
    .height(1080)                    // default: 480
    .fps(60)                         // default: 30
    .bind_ip([127, 0, 0, 1].into())  // default: 127.0.0.1
    .port(3956)                      // default: 3956
    .pixel_format(viva_fake_gige::RGB8)  // default: MONO8
    .zip_xml(true)                   // default: false
    .enforce_heartbeat(true)         // default: false
    .heartbeat_timeout_ms(3_000)     // default: the device's own value
    .build()
    .await
    .unwrap();
```

The last three are there to reproduce behaviour real cameras have and fakes
usually do not:

- `zip_xml` serves the GenApi document ZIP-compressed, which many vendors do and
  which exercises a decompression path that would otherwise never be tested.
- `enforce_heartbeat` makes the camera actually revoke control privilege when no
  GVCP command arrives inside `heartbeat_timeout_ms`. Without it, a client that
  forgets to send keepalives passes every test and fails on real hardware.

Image dimensions, exposure and gain can also be changed at runtime through
GenApi writes — the fake responds to `Width`, `Height`, `ExposureTime` and
`Gain` the way a real camera does.
