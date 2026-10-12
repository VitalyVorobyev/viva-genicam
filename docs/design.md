# System Design

Contributor-facing description of how viva-genicam is put together and why.
User-facing documentation lives in the mdBook (`book/`); decision history in
`docs/adrs/`.

## Goals & scope

- **Pure-Rust GenICam stack**: GigE Vision and USB3 Vision implemented from the
  published EMVA specifications, no C dependencies on the core path
  (see [ADR-0011](adrs/adr0011-pure-rust-genicam-stack.md)).
- **Industrial target**: machine-vision applications that need deterministic
  control, sustained streaming, and defensive handling of untrusted network
  input.
- **Pre-1.0**: no backward-compatibility guarantees. Clear design and structure
  take priority over API stability, and breaking releases are expected — in
  practice every minor so far has become breaking because a user's camera needed
  it rather than because we planned the window.

## Layered architecture

Strict layering, bottom to top (see
[ADR-0012](adrs/adr0012-layered-crate-architecture.md)):

```
viva-service / viva-service-u3v  - Zenoh bridges: GigE / U3V cameras → GUI consumers
viva-camctl                      - CLI binary (crates.io + prebuilt release binaries)
viva-pygenicam                   - Python bindings (PyO3, PyPI wheels)
        ↓
viva-genicam (facade)   - End-user API: Camera<T>, discovery, streaming
        ↓
viva-genapi             - GenApi engine: NodeMap, node evaluation, caching
        ↓
viva-genapi-xml         - XML parsing: GenICam XML → XmlModel IR
        ↓
viva-gige / viva-u3v    - Transports: GVCP/GVSP over UDP, USB3 Vision bulk
        ↓
viva-gencp              - Protocol primitives: GenCP encode/decode
```

**Supporting crates:**

- `viva-pfnc` — Pixel Format Naming Convention tables; the single authority for
  pixel-format codes and layouts, and the one pixel conversion
  (`viva_pfnc::convert`) the library, Python and Viva Studio share.
- `viva-sfnc` — Standard Feature Naming Convention constants.
- `viva-zenoh-api` — the message payloads and topic names the services
  publish. This is the contract, not the transport: despite the name it does
  not depend on the `zenoh` crate, so a client can share the message format
  without linking a broker, and it compiles everywhere including wasm. Its
  optional `genapi` feature converts `viva-genapi`'s `FeatureState` into the
  wire form, so every service that reads features builds that payload the same
  way.

**Test crates (not published):**

- `viva-fake-gige` / `viva-fake-u3v` — in-process fake cameras, the primary
  test vehicle (see Testing strategy below).

The `studio/` GUI workspace (Viva Studio, a Tauri desktop app) lives in this
repository as a second Cargo workspace (ADR-0017). It is the library's
reference viewer (ADR-0024): in embedded mode, the primary path, it drives the
camera in-process through `viva-genicam`; in remote mode it consumes
`viva-zenoh-api` and talks to the services via Zenoh.

Layering rules: a crate depends only on layers below it. `viva-genapi-xml` and
`viva-genapi` have no transport dependency and compile for
`wasm32-unknown-unknown`, which is what lets the studio browse XML offline via
`NullIo`. All workspace crates plus the Python package share one release
version.

## Key abstractions

**`RegisterIo` trait** (`viva-genapi`) — the core register read/write
abstraction, deliberately *synchronous*
(see [ADR-0014](adrs/adr0014-sync-registerio-async-adapters.md)).
Implementations:

- `GigeRegisterIo` — async-to-sync adapter over `GigeDevice`, callable from
  any context: a plain thread, a multi-thread runtime (`block_in_place`) or a
  current-thread one (a scoped thread). A camera opened from a current-thread
  runtime, or through `viva_genicam::blocking`, carries a one-worker runtime
  of its own that the adapter shuts down on drop
  ([ADR-0027](adrs/adr0027-blocking-api-and-transport-runtimes.md)). Also
  **owns the control session**: constructing one for a device
  that claimed control spawns a task that reads `GevHeartbeatTimeout` and
  refreshes the device's timer at a quarter of it, and dropping one cancels
  and drains its I/O before runtime shutdown, then releases control (CCP ← 0)
  within 500 ms. A runtime the caller lent must outlive the transport. Holding the
  transport is therefore sufficient to hold control privilege — nothing above
  this layer runs a heartbeat. Its `control_state` (`Held`, `Monitor`, `Lost`,
  `Released`, watchable) reports privilege lost to an expired heartbeat, or to
  another application on a camera that names its controller;
  `reclaim_control` takes it back. Only a session in `Held` writes. A *monitor*
  (`GigeRegisterIo::monitor`, `Access::Monitor`) claims nothing, sends no
  heartbeat and refuses writes before they reach the wire. While a register operation holds the device
  mutex, the GVCP receive loop itself sends CCP probes and drains their ACKs
  by request ID. The background task covers idle time; the socket owner covers
  in-flight waits ([ADR-0023](adrs/adr0023-inflight-gvcp-heartbeats.md)).
- `MockIo` — in-memory register map for tests.
- `NullIo` — no-op backend for offline XML browsing (studio, wasm).

**`Camera<T>`** (`viva-genicam`) — a `NodeMap` and a `RegisterIo`. Features
are read and written by type (`get_integer`, `set_float`, ...) or as text
(`get`/`set`, for a command line); a category has no value and a command is
executed, never set. The standard-feature helpers (`set_exposure_time`,
`set_gain`, `enable_trigger`, `set_roi`, `set_binning`, `set_frame_rate`,
`load_user_set`, ...) resolve each feature through a table of the names
cameras declare it under (`StandardFeature`, names in `viva-sfnc`), in the
vendor corpus's order of prevalence, and report which they used.
`acquisition_start`/`stop` set and clear `TLParamsLocked`.
`save_features`/`load_features` save the features the XML marks
`<Streamable>` to a file and restore them; what is saved, the walk over
selectors and the restore order are `viva-genapi`'s `persistence`, which does
no I/O ([ADR-0028](adrs/adr0028-configuration-persistence.md)). What only GigE
Vision has — events, multicast, streams, control privilege — is in
`impl Camera<GigeRegisterIo>`, so it does not compile against a USB3 Vision
camera.

**`NodeMap`** (`viva-genapi`) — parsed from GenICam XML, stores nodes by name,
tracks the dependency graph for cache invalidation. Supports `pValue`
delegation: Integer/Float/Enum/Boolean/Command nodes can delegate to `IntReg`
or other backing nodes. The `Node` enum covers Integer, Float, Enum, Boolean,
Command, Category, SwissKnife, Converter, IntConverter, String, Register.
Introspection API (`node_names()` in document order, `dependents()`,
`selectors_of()`, `categories()`, `identity()`, `kind_name()`,
`access_mode()`, `ports()`, `port_features()`) serves external
consumers such as the studio.

**Ports** (`viva-genapi`) — every register reaches its bytes through the
`<Port>` its `<pPort>` names ([ADR-0026](adrs/adr0026-genapi-ports-chunks-events.md)).
The device port is the `RegisterIo`. A chunk port (`<ChunkID>`) reads one chunk
of the frame attached with `NodeMap::attach_chunks`, an event port
(`<EventID>`) the event attached with `attach_event`; their addresses count from
the start of that chunk or event, and a negative chunk address from its end.
Nothing attached is `NotAttached`, never a device read; writes are refused;
attaching invalidates everything that depends on the port. The facade's
`Camera::with_chunks` / `with_event` attach, read and detach.

**`GigeDevice`** (`viva-gige`) — async UDP wrapper for GVCP discovery/control
and GVSP streaming. Uses the proper GVCP wire format (0x42 key byte, 4-byte
addresses).

**`FrameStream` / `U3vFrameStream`** (`viva-genicam`) — frame streams. The
GigE side is the GVSP receive engine
([ADR-0025](adrs/adr0025-gvsp-receive-engine.md)): one thread per stream reads
the socket, accepting only the camera as a source, a socket-free assembler
builds blocks in place and finds the gaps to ask the camera for again, and
frames reach the application through a four-frame drop-oldest queue, async or
blocking; opened by `Camera::open_stream`. The U3V side wraps blocking USB bulk reads via
`spawn_blocking` + an mpsc channel, created through `U3vStreamBuilder` or
`U3vFrameStream::start()`.

**`DeviceHandle`** (`viva-service`) — wraps `Camera<GigeRegisterIo>` in
`spawn_blocking` for async-safe access from Zenoh queryable handlers, and owns
reconnection with exponential backoff. Reconnection needs no keepalive
coordination: the replacement camera brings its own, and the old one retires
when the swap drops the camera that owned it. `U3vDeviceHandle<T>`
(`viva-service-u3v`) is generic over `T: UsbTransfer`, so the same service code
runs against `FakeU3vTransport` (`--fake` mode) and `RusbTransfer` (real USB).

## Data flows

**Control path** — `camera.set("ExposureTime", …)` → `NodeMap` resolves the
feature (pValue delegation, selectors, converters) into register operations →
`RegisterIo` → GVCP (GigE) or GenCP-over-USB (U3V) → device. Writes invalidate
dependent cached values via the NodeMap dependency graph.

**Streaming path** — device emits GVSP packets (GigE) or bulk transfers (U3V)
→ `viva-gige` parses each datagram in place, the receive engine's assembler
places it in its block's buffer → `Frame` with block id, status and
pixel-format metadata from `viva-pfnc`, counted in `StreamStats` → application
via `FrameStream`/`U3vFrameStream`. A block that carries chunks is split into
the image and its chunks by ID (`viva_gige::gvsp::parse_chunks`, read from the
end); their values are read through the nodemap's chunk ports, never decoded
here.

**Service path** — camera → `viva-service`/`viva-service-u3v` (`DeviceHandle`)
→ Zenoh (types from `viva-zenoh-api`) → studio or other subscribers. The
service owns discovery announcements, XML serving, node read/write, and frame
publication.

## Error handling policy

Each crate defines its own typed error enum; errors crossing crate boundaries
carry their source (`#[source]` chains) rather than flattening into strings.
Existing `Transport(String)`-style variants are acknowledged debt, tracked as
API-03 in [roadmap](roadmap.md) Phase 5. Panics must never
cross a public API in response to remote input — a malformed camera XML or a
hostile packet is an `Err`, not a crash.

## Testing strategy

In-process fake cameras (`viva-fake-gige`, `viva-fake-u3v`) are the primary
strategy: no hardware, CI-friendly, deterministic, and fast enough to run the
full integration suite on every push
(see [ADR-0013](adrs/adr0013-fake-camera-first-testing.md)).

**Realism policy**: a fake must implement the *standard's* semantics, not
mirror the implementation's assumptions — fake and client that share a wrong
assumption pass every test together
([ADR-0013](adrs/adr0013-fake-camera-first-testing.md),
[ADR-0019](adrs/adr0019-transport-conformance-and-spec-derived-fakes.md) record
the cases). Consequences:

- Fake behavior is derived from the spec text, not from what our receiver
  happens to send.
- Wire formats are pinned by spec-derived golden byte fixtures, asserted
  independently of the client parser — a round trip through our own code proves
  only that the two agree.
- Tests assert payload sizes and content, not just headers and status codes,
  and identity assertions check values rather than presence.
- A protocol feature with no fake implementation is not considered implemented.
- Predicates and test fixtures wire onto real SFNC features; no synthetic
  `Test*` nodes in fake-camera XML.

## Design tenets

1. **Clear API boundaries** — curated re-exports; no blanket `pub mod` of
   internals.
2. **SOLID, applied pragmatically** — small traits (`RegisterIo`,
   `UsbTransfer`) as seams; open for extension via traits, not boolean flags.
3. **DRY** — one implementation per concern: a single frame-reassembly path,
   pixel formats and their conversion owned by `viva-pfnc`.
4. **YAGNI** — no speculative scaffolding; dead code gets deleted or wired up
   (the GVSP resend request that sat unused, in the wrong layout, until 0.9 is
   the cautionary tale).
5. **Spec-conformance over self-consistency** — tests validate against the
   standard, not against our own second implementation of the same mistake.
6. **Errors carry sources; panics never cross a public API on remote input.**

When a change conflicts with a tenet, write or update an ADR in `docs/adrs/`
rather than silently deviating.
