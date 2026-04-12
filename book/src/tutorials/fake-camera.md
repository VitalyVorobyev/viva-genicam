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

Expected output:

```
Starting fake GigE Vision camera on 127.0.0.1:3956 ...
  Fake camera is running.

Discovering cameras (2 s timeout) ...
  Found 1 device(s):
    IP: 127.0.0.1  Model: FakeGigE  Manufacturer: viva-genicam

Connecting to 127.0.0.1 ...
  Connected. GenApi XML: 5788 bytes, 20 features.

Reading camera features:
  Width = 640
  Height = 480
  PixelFormat = Mono8
  ExposureTime = 5000
  Gain = 0
  GevTimestampTickFrequency = 1000000000

Setting Width = 320, ExposureTime = 10000 ...
  Width readback = 320

Streaming 5 frames ...
  Frame 1: 320x480 Mono8 payload=153600B ts=7393542
  Frame 2: 320x480 Mono8 payload=153600B ts=113549417
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

All integration tests use the fake camera automatically:

```bash
# Full workspace test suite (includes fake camera tests)
cargo test --workspace

# Just the camera integration tests (12 tests)
cargo test -p viva-genicam --test fake_camera

# Zenoh service end-to-end tests (3 tests)
cargo test -p viva-service --test fake_camera_e2e
```

## Using the fake camera in your own code

Add `viva-fake-gige` as a dev-dependency:

```toml
[dev-dependencies]
viva-fake-gige = { git = "https://github.com/VitalyVorobyev/viva-genicam" }
```

Start a fake camera in your test:

```rust,no_run
use viva_fake_gige::FakeCamera;

#[tokio::test]
async fn my_camera_test() {
    // Start a fake camera on loopback
    let _camera = FakeCamera::builder()
        .width(1024)
        .height(768)
        .fps(15)
        .bind_ip([127, 0, 0, 1].into())
        .port(3956)
        .build()
        .await
        .expect("failed to start fake camera");

    // Now use viva_genicam::gige::discover_all() to find it,
    // connect_gige() to connect, and FrameStream to stream.
}
```

## Running as a standalone server

The `viva-fake-gige` binary starts a long-running fake camera that stays alive
until Ctrl+C. This is the recommended way to test interactively with
`viva-camctl` or `viva-service` + `genicam-studio`.

```bash
# Terminal 1: start the fake camera
cargo run -p viva-fake-gige

# Custom dimensions and frame rate
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15
```

Output:
```
Fake camera running on 127.0.0.1:3956 (640x480 Mono8 @ 30 fps)
Press Ctrl+C to stop.
```

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

## E2E testing with genicam-studio

The full stack test uses 3 terminals:

```bash
# Terminal 1: fake camera
cargo run -p viva-fake-gige

# Terminal 2: camera service (bridges camera to Zenoh)
# The --zenoh-config is required so studio can connect via TCP
cargo run -p viva-service -- \
  --iface lo0 \
  --zenoh-config ../genicam-studio/config/zenoh-local.json5
# On Linux: --iface lo

# Terminal 3: studio app (auto-loads config/zenoh-studio.json5)
cd ../genicam-studio/apps/genicam-studio-tauri
cargo tauri dev
```

The studio will discover the fake camera, show its feature tree, and stream
gradient images in the viewer. See
[genicam-studio/docs/manual-e2e-test.md](https://github.com/VitalyVorobyev/genicam-studio)
for the full test checklist.

## Fake camera configuration

The `FakeCameraBuilder` supports:

```rust,no_run
# use viva_fake_gige::FakeCamera;
# async fn example() {
let camera = FakeCamera::builder()
    .width(1920)       // Image width (default: 640)
    .height(1080)      // Image height (default: 480)
    .fps(60)           // Target frame rate (default: 30)
    .bind_ip([127, 0, 0, 1].into())  // Bind address (default: 127.0.0.1)
    .port(3956)        // GVCP port (default: 3956)
    .build()
    .await
    .unwrap();
# }
```

Image dimensions and exposure time can also be changed at runtime through
GenApi register writes -- the fake camera responds to Width, Height,
ExposureTime, and Gain register writes just like a real camera.
