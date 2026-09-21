# Testing Cookbook: Real Camera Service Integration

This document describes how to test GenICam Studio with the **real camera service** (`viva-service`) from this repository, using `arv-fake-gv-camera` as a simulated GigE Vision device.

## Prerequisites

| Component | Location | Install |
|-----------|----------|---------|
| viva-genicam workspace | repository root | — |
| aravis (fake camera) | system / `../aravis` | `brew install aravis` |
| Viva Studio | `studio/` | — |

Verify aravis is installed:
```bash
which arv-fake-gv-camera-0.8  # should print a path
```

## Architecture Overview

```
┌──────────────────┐     Zenoh      ┌──────────────────┐     GVCP/GVSP    ┌──────────────────┐
│  GenICam Studio   │ ◄──────────► │  viva-service  │ ◄──────────────► │ arv-fake-gv-cam  │
│  (Tauri app)      │   pub/sub    │  (Rust binary)    │    UDP/GigE      │ (aravis sim)     │
└──────────────────┘   queryable   └──────────────────┘                   └──────────────────┘
```

The **mock service** (`studio/apps/viva-mock-service`) can be swapped 1:1 with `viva-service` — they implement the same Zenoh API contract (`docs/studio/zenoh-api.md`).

## Quick Start: Mock Service (no camera needed)

This is the fastest way to develop and test the UI:

```bash
# Terminal 1: start mock service (from studio/)
cd studio
cargo run -p viva-mock-service -- --zenoh-config config/zenoh-local.json5

# Terminal 2: start studio (from the repository root)
export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri
cargo tauri dev
```

Both configs are required. The mock service defaults to `zenoh::Config::default()`
— peer mode with multicast scouting — and Studio without `ZENOH_CONFIG` is not a
Zenoh client at all, it is in embedded mode talking to real hardware. Pointing
both at the fixed loopback endpoint is the same topology the real-service recipe
below uses, and the one `tests/e2e` exercises.

The mock service generates synthetic test patterns (Mono8 gradient, RGB8 color bars) — no real camera required.

## Quick Start: Real Service + Fake Camera

```bash
# Terminal 1: start the fake GigE camera
# On macOS — use loopback (works) or a real NIC:
arv-fake-gv-camera-0.8 -i 127.0.0.1

# Terminal 2: start the real camera service (from the repo root)
cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5
# Or: --iface lo   # Linux loopback
# Or: --iface en0  # real NIC

# Terminal 3: start studio. ZENOH_CONFIG is required and must be absolute,
# because the `cd` that follows would break a relative path.
export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri
cargo tauri dev
```

`zenoh-studio.json5` connects to the service endpoint configured by
`zenoh-local.json5`. Studio does not load it on its own — there is no default
path and no dev-mode auto-detection. Without `ZENOH_CONFIG` it starts in
embedded mode, whose discovery skips loopback, so the fake camera never appears
(#132). The header chip shows which mode is active.

## What to Expect

### Discovery
- Within 5 seconds, studio should show the fake camera in the device list
- Device ID: `cam-000000000000` (fake camera uses all-zero MAC)
- Model: "Fake" / Manufacturer: "Aravis"

### Connection
- Click to connect — studio fetches GenICam XML and renders the feature tree
- Available features: Width, Height, PixelFormat, ExposureTimeAbs, Gain, AcquisitionMode, TriggerMode, etc.
- Width/Height default: 512x512 (aravis fake camera defaults)

### Feature Read/Write
- Change Width, Height — readback should reflect new values
- Change PixelFormat (Mono8, RGB8, BayerRG8, Mono16) — affects streaming pixel data
- ExposureTimeAbs — adjustable float value

### Streaming (acquisition)
- Start acquisition — studio should receive frames via the WebSocket streamer
- Frame dimensions match the camera's Width/Height
- PixelFormat changes are reflected in the stream
- FPS is published via `acquisition/status` (~30 fps from fake camera)
- Stop acquisition — frames stop arriving

## Zenoh Key Reference

All keys prefixed with `genicam/devices/{device_id}/`:

| Key suffix | Direction | Description |
|------------|-----------|-------------|
| `announce` | Service → App | Periodic device announcement (every 2s) |
| `xml` | App → Service (query) | Full GenICam XML |
| `status` | Service → App | Connection status |
| `nodes/{name}/value` | Service → App | Node value updates (published on connect and after writes) |
| `nodes/{name}/set` | App → Service (query) | Write node value |
| `nodes/{name}/execute` | App → Service (query) | Execute command node |
| `nodes/bulk/read` | App → Service (query) | Batch read node values |
| `acquisition/control` | App → Service (query) | Start/stop acquisition |
| `acquisition/status` | Service → App | Acquisition state with FPS |
| `image` | Service → Streamer | Binary: 16-byte FrameHeader + raw pixels |
| `image/meta` | Service → App | JSON: pixel_format, width, height, payload_size |

Full specification: [`docs/studio/zenoh-api.md`](zenoh-api.md)

## Differences: Mock Service vs Real Service

| Aspect | Mock Service | Real Service (viva-service) |
|--------|-------------|-------------------------------|
| Camera source | Synthetic patterns | Real GigE Vision device (or aravis fake) |
| Node values | Simulated in-memory | Read from actual camera registers |
| Frame data | Generated gradients/bars | Real GVSP reassembled frames |
| Node interdependencies | Simulated (MS-12) | Driven by actual camera register dependencies |
| Pixel formats | Mono8, Mono16, BayerRG8, RGB8 | Whatever the camera supports |
| Error behavior | Always succeeds | May return transport errors |
| XML source | Built-in fixture | Fetched from camera via GVCP |
| Initial node values | Published from config | Published for common SFNC features on connect |
| Device lost | Never | Detected when device disappears from discovery |

**From the app's perspective, both services are interchangeable.** The Zenoh API contract is identical.

## viva-service Capabilities (Apr 2026)

The real service (`crates/viva-service`) supports:

- **Discovery**: periodic GVCP broadcast, loopback support, multi-camera dedup
- **CCP**: claims Control Channel Privilege on connect (required for streaming)
- **XML**: serves raw GenICam XML fetched from the camera
- **Nodes**: read/write/execute/bulk-read via Zenoh queryables; initial SFNC values published on connect
- **Acquisition**: start/stop via AcquisitionStart/Stop commands; frame streaming with FrameHeader encoding
- **FPS tracking**: measured FPS published in AcquisitionStatus every second
- **Device lost**: detects when camera disappears from discovery, cleans up tasks
- **XML parsing**: full pValue delegation, IntReg, IntSwissKnife (hex literals), StructReg, Converter

## Library Integration Tests

All 12 integration tests pass against `arv-fake-gv-camera` on macOS (including loopback streaming). From the repo root:

```bash
cargo test -p viva-genicam --test fake_camera -- --ignored --test-threads=1
```

Tests cover: discovery, connection, XML fetch, feature read/write, command execution, frame streaming, frame dimension validation, full lifecycle.

## Automated E2E Tests

The `studio/tests/e2e/` crate runs automated tests that spawn the fake camera and service as child processes.

```bash
# Build the service first (from the repo root)
cargo build -p viva-service

# Run all E2E tests (discovery, node read/write, acquisition, device lost) — from studio/
GENICAM_SERVICE_PATH=../target/debug/viva-service \
  cargo test -p e2e-tests --test e2e -- --ignored --test-threads=1

# Streamer integration test (no external binaries needed) — from studio/
# Publishes synthetic frames on Zenoh, verifies BMP arrives over WebSocket
cargo test -p e2e-tests --test streamer_e2e
```

### Test coverage

| Test | What it verifies |
|------|-----------------|
| `test_discovery_and_xml_fetch` | Device announce, XML download, UiGraph parsing |
| `test_node_read_write` | Bulk read, write Width=320, readback, restore |
| `test_acquisition_frames` | Start/stop acquisition, frame header decode (best-effort in the default harness) |
| `test_manual_topology_frames_and_ws_stream` | Manual TCP-configured topology: service `zenoh-local.json5`, client `zenoh-studio.json5`, raw image + BMP over WebSocket |
| `test_device_lost_detection` | Kill camera, verify disconnect status (graceful on loopback) |
| `test_streamer_synthetic_frame` | Full Zenoh→FrameHeader→BMP→WebSocket pipeline |

### Known limitations

- **Default Zenoh peer-mode harness**: Early image samples can still be missed while peer topology and subscription interest settle. Use `test_manual_topology_frames_and_ws_stream` as the authoritative macOS/manual-topology check; it mirrors the real Studio setup with `zenoh-local.json5` + `zenoh-studio.json5`.
- **Port conflicts**: Tests run with `--test-threads=1` because the fake camera binds to UDP port 3956.

## Troubleshooting

### No device discovered
- Check that the fake camera is running: `arv-fake-gv-camera-0.8 -i 127.0.0.1`
- If using `--iface`, ensure it matches the camera's subnet
- Verify on macOS loopback with: `arv-tool-0.8 --gv-discovery-interface=lo0`
- Ensure no firewall blocks UDP broadcast on port 3956

### Connection fails (XML parse error)
- Check `viva-service` logs for specific parsing errors
- The service supports all node types used by the aravis fake camera XML

### No frames during acquisition
- Service needs CCP (Control Channel Privilege) — this is handled automatically
- Check service logs for `first GVSP frame received` and `published first image frame to Zenoh`
- Check streamer logs for `First image/meta received`, `First raw image frame received`, and `First BMP frame published to WebSocket broadcaster`
- If the service has been idle on macOS loopback before start, make sure you're using a current `viva-service` build (`cargo build -p viva-service` from the repo root)
- Verify the streamer (`viva-ws-streamer`) is running if testing through the studio UI

### Node write returns error
- Some nodes are read-only (SensorWidth, SensorHeight)
- Value out of range — check Min/Max constraints
- Node not found — may be skipped during XML parsing (check logs)
