# Testing Cookbook: Studio against the in-repo fake camera

How to exercise Viva Studio without hardware, using the in-repo fake GigE
camera (`viva-fake-gige`) behind the real camera service (`viva-service`), or
the mock service. Nothing outside this repository is needed.

## Two modes, and which one a fake can reach

Studio runs in one of two modes, shown by the chip in its header:

- **Embedded** (no `ZENOH_CONFIG`): Studio drives the camera in-process through
  `viva-genicam`. This is how users run it against real cameras, but its
  discovery skips loopback, so it never finds the fake camera.
- **Remote** (`ZENOH_CONFIG` set): Studio is a Zenoh client of `viva-service`,
  which owns the camera. This is the mode the recipes below use.

```
┌──────────────────┐     Zenoh      ┌──────────────────┐   GVCP/GVSP    ┌──────────────────┐
│   Viva Studio    │ ◄────────────► │   viva-service   │ ◄────────────► │  viva-fake-gige  │
│   (Tauri app)    │ pub/sub, query │   (Rust binary)  │  UDP loopback  │ (in-repo fake)   │
└──────────────────┘                └──────────────────┘                └──────────────────┘
```

The **mock service** (`studio/apps/viva-mock-service`) stands in for
`viva-service` and the camera together. It is a third copy of the Zenoh
contract and already behind it (backlog ST-02), so prefer the real service.

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
# Terminal 1: start the fake GigE camera (from the repo root)
cargo run -p viva-fake-gige
# Options: --width 512 --height 512 --fps 15 --pixel-format rgb8

# Terminal 2: start the real camera service (from the repo root)
cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5
# Or: --iface lo   # Linux loopback

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
- Studio lists the fake camera within a few seconds.
- Device ID `cam-deadbeefcafe` (MAC `DE:AD:BE:EF:CA:FE`), manufacturer
  `viva-genicam`, model `FakeGigE`, serial `FAKE-001`, user name `FakeCamera`.
  A second fake needs its own `--mac`.

### Connection
- Connecting fetches the GenICam XML and renders the feature tree.
- Features include Width, Height, OffsetX/Y, PixelFormat, ExposureTime, Gain,
  BlackLevel, AcquisitionMode, AcquisitionFrameRate and the transport-layer
  registers.
- Width/Height default to 640×480 (the fake's `--width`/`--height`).

### Feature Read/Write
- Change Width and Height; the read-back reflects the new values.
- Change PixelFormat; the stream follows it.

### Streaming (acquisition)
- Start acquisition; frames arrive over the WebSocket streamer.
- Frame dimensions match Width/Height, and PixelFormat changes show up in the
  stream.
- `acquisition/status` reports the measured FPS (30 by default, `--fps`).
- Stop acquisition; frames stop.

## Zenoh Key Reference

All keys prefixed with `genicam/devices/{device_id}/`:

| Key suffix | Direction | Description |
|------------|-----------|-------------|
| `announce` | Service → App | Periodic device announcement (cadence: backlog SVC-01) |
| `xml` | App → Service (query) | Full GenICam XML |
| `status` | Service → App | Connection status |
| `nodes/{name}/value` | Service → App | Node value updates (published on connect and after writes) |
| `nodes/{name}/set` | App → Service (query) | Write node value |
| `nodes/{name}/execute` | App → Service (query) | Execute command node |
| `nodes/bulk/read` | App → Service (query) | Batch read node values |
| `nodes/{name}/state` | App → Service (query) | One node's `FeatureState` (ADR-0010) |
| `nodes/bulk/state` | App → Service (query) | `FeatureState` for many nodes |
| `acquisition/control` | App → Service (query) | Start/stop acquisition |
| `acquisition/status` | Service → App | Acquisition state with FPS |
| `image` | Service → Streamer | Binary: 16-byte FrameHeader + raw pixels |
| `image/meta` | Service → App | JSON: pixel_format, width, height, payload_size |
| `evs` | Service → subscribers | Binary: 24-byte EvsHeader + encoded events (event-vision cameras only) |

Full specification: [`docs/studio/zenoh-api.md`](zenoh-api.md)

## Differences: Mock Service vs Real Service

| Aspect | Mock Service | Real Service (viva-service) |
|--------|-------------|-------------------------------|
| Camera source | Synthetic patterns | `viva-fake-gige` or a real GigE Vision device |
| Node values | Simulated in-memory | Read from the camera's registers |
| Frame data | Generated gradients/bars | GVSP frames, reassembled |
| Node interdependencies | Simulated (MS-12) | The camera's own register dependencies |
| Pixel formats | Mono8, Mono16, BayerRG8, RGB8 | Whatever the camera supports |
| Error behavior | Always succeeds | May return transport errors |
| XML source | Built-in fixture | Fetched from the camera over GVCP |
| `FeatureState` topics | Not implemented | `nodes/{name}/state`, `nodes/bulk/state` |
| Device lost | Never | Detected when the device stops announcing |

## Automated tests

What runs today, and where:

| Suite | Where | Runs in CI |
|-------|-------|------------|
| Library integration suites (`fake_camera`, `predicates`, `heartbeat`, …) | repo root, see `CLAUDE.md` → Commands | yes (`ci.yml`) |
| `viva-service --test fake_camera_e2e` | repo root | yes (`ci.yml`) |
| Studio workspace unit tests (`viva_xml_model`, `viva_streamer`, mock service) | `studio/`: `cargo test --workspace` | yes (`studio-ci.yml`) |
| `streamer_e2e`: synthetic frames over Zenoh, BMP over WebSocket | `studio/`: `cargo test -p e2e-tests --test streamer_e2e` | yes (`studio-ci.yml`) |
| Tauri backend unit tests | `studio/apps/viva-studio-tauri/src-tauri`: `cargo test` | yes, macOS only (`studio-ci.yml`) |
| UI tests (vitest): pure functions in node, components on jsdom | `studio/ui/viva-studio-ui`: `bun run test -- --run` | yes (`studio-ci.yml`) |
| `e2e` and `feature_state` | `studio/tests/e2e` | **no** — `#[ignore]`d, and the harness still targets aravis's `arv-fake-gv-camera-0.8` and a pre-rename service path. Backlog ST-03 replaces them with backend tests on `viva-fake-gige` |

Every fake-camera suite binds UDP 3956, so two of them running at once — for
example in two worktrees — cross-talk and fail spuriously.

### UI component tests

`*.test.ts` files run in node and test pure functions. `*.test.tsx` files run
on jsdom with Testing Library (`vite.config.ts` splits them into two vitest
projects), and talk to a mocked backend through Tauri's own IPC mock:

```tsx
import { mockIPC } from "@tauri-apps/api/mocks";

mockIPC((cmd, args) => {
  if (cmd === "write_node") throw new Error("node is locked");
  return undefined;
});
```

`src/test/setup-dom.ts` clears the mock after every test. `src/test/model.ts`
builds a `ParseXmlResponse` from a few partial nodes, and `src/test/bmp.ts`
builds frames shaped like the streamer's. A component that calls `useToast`
needs a `ToastProvider` around it, and a `ToastContainer` if the test reads
the toast. A regression test must fail against the unfixed code: say in the PR
how you saw it fail.

## Troubleshooting

### No device discovered
- Check the header chip. In embedded mode the fake camera is never discovered;
  set `ZENOH_CONFIG` as in the recipe above.
- Check that `viva-fake-gige` is running and that `viva-service --iface` names
  the loopback interface (`lo0` on macOS, `lo` on Linux).
- Ensure no firewall blocks UDP broadcast on port 3956.

### Connection fails (XML parse error)
- Check the `viva-service` log. A node the library cannot handle is skipped
  and logged rather than failing the document; a document-level failure is a
  bug worth reporting with the XML attached.

### No frames during acquisition
- Check the service log for `first GVSP frame received` and
  `published first stream block to Zenoh`.
- Check Studio's log for `First image/meta received`,
  `First raw image frame received` and
  `First BMP frame published to WebSocket broadcaster`.

### Node write returns error
- Some nodes are read-only (SensorWidth, SensorHeight).
- The value may be out of range; check the node's Min/Max.
- A node missing from the tree may have been skipped during XML parsing (see
  the log).
