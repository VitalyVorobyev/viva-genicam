# Testing Cookbook: Studio against the in-repo fake camera

How to exercise Viva Studio without hardware, using the in-repo fake GigE
camera (`viva-fake-gige`) behind the real camera service (`viva-service`).
Nothing outside this repository is needed.

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

## Quick Start: Service + Fake Camera

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

## Zenoh keys

The keys the service publishes and answers, and their payloads, are in
[`zenoh-api.md`](zenoh-api.md).

## Automated tests

What runs today, and where:

| Suite | Where | Runs in CI |
|-------|-------|------------|
| Library integration suites (`fake_camera`, `predicates`, `heartbeat`, …) | repo root, see [`CONTRIBUTING.md`](../../CONTRIBUTING.md#tests) | yes (`ci.yml`) |
| `viva-service --test fake_camera_e2e` | repo root | yes (`ci.yml`) |
| Studio workspace unit tests (`viva_xml_model`, `viva_streamer`) | `studio/`: `cargo test --workspace` | yes (`studio-ci.yml`) |
| Tauri backend tests, both modes end to end against `viva-fake-gige`: embedded in-process, and remote through `viva-service` run in-process on a shared Zenoh session (`backend/embedded.rs`, `backend/remote.rs`, `main.rs`) | `studio/apps/viva-studio-tauri/src-tauri`: `cargo test` | yes, on macOS, Ubuntu and Windows (`studio-ci.yml`) |
| UI tests (vitest): pure functions in node, components on jsdom | `studio/ui/viva-studio-ui`: `bun run test -- --run` | yes (`studio-ci.yml`) |

Every fake-camera suite binds UDP 3956, so two of them running at once — for
example in two worktrees — cross-talk and fail spuriously. Within the Tauri
crate the tests take `backend::embedded::fake_camera::lock()` first.

### Backend tests

`backend::embedded::fake_camera` starts a fake on loopback and checks what the
camera itself saw (`fake.commands()`, `fake.peek(...)`); `first_frame` reads the
first BMP from the viewer's WebSocket. A remote-mode test runs the service the
way its binary does, with `viva_service::serve::serve_device` on the test's own
Zenoh session, and hands the same session to
`RemoteBackend::start_with_session`. `state::events::Recorder` stands in for the
webview and keeps every event the backend sends.

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

A component that reads live state needs a `FeatureStoreProvider connected`
around it, and answers `query_feature_states_bulk` from the mock; pass
`{ shouldMockEvents: true }` to `mockIPC` and `emit` from
`@tauri-apps/api/event` to deliver `feature-state-changed` or
`features-invalidated` (`FeatureStoreContext.test.tsx`).

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
- Check the service log for `first frame received` and
  `published first stream block to Zenoh`.
- Check Studio's log for `First image/meta received`,
  `First raw image frame received` and
  `First BMP frame published to WebSocket broadcaster`.

### Node write returns error
- Some nodes are read-only (SensorWidth, SensorHeight).
- The value may be out of range; check the node's Min/Max.
- A node missing from the tree may have been skipped during XML parsing (see
  the log).
