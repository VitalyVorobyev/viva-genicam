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
viva-gentl                       - GenTL producer: a .cti for GenTL consumers (release asset)
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
  publish; pixel formats are `viva-pfnc`'s. This is the contract, not the transport: despite the name it does
  not depend on the `zenoh` crate, so a client can share the message format
  without linking a broker, and it compiles everywhere including wasm. Its
  optional `genapi` feature converts `viva-genapi`'s `FeatureState` into the
  wire form, so every service that reads features builds that payload the same
  way.

`viva-gentl` is a C ABI over the facade (ADR-0031): it uses only
`viva-genicam`'s public API and runs no GenApi of its own — the consumer does
— so each module's XML and register map are its own tables, and the remote
port is the camera's `GigeRegisterIo`. It is not published to crates.io; the
release attaches it as `viva_gentl-<target>.cti`.

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

Values are cached as the XML's `<Cachable>` allows
([ADR-0029](adrs/adr0029-caching-polling-and-refresh.md)): `NoCache` never,
`WriteAround` (a `<FloatReg>`'s default) only what was read, `WriteThrough`
(every other register's) what was written too; a node is cached only as far
as every register its value is read through allows. A `<PollingTime>`
expires a cached value, measured against an injected `Clock` so the crate
still runs on `wasm32-unknown-unknown`. Every write and invalidation
advances a generation: `changed_since(generation)` names what changed, and
`poll()` invalidates the values whose polling time has passed — a query in
place of callbacks, because the node map is not `Sync`.

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
blocking; opened by `Camera::open_stream`. The U3V side
([ADR-0032](adrs/adr0032-usb3-vision-transport.md)) is the same shape: one
thread per stream keeps several bulk transfers queued on the endpoint
(`viva_u3v::U3vStream`, over the `BulkIn` queue a `UsbTransfer` hands out),
plans each from where the block stands — leader, payload, trailer — and
replans when the camera's transfers diverge from that plan; a failed transfer
costs its block, not the stream, and a gone device ends it with
`U3vError::Disconnected` and marks the camera lost. Frames, counters and the
drop-oldest queue are the GigE side's own (`StreamStats`, `DeliveryQueue`);
opened by `Camera::open_stream(U3vStreamOptions)` or `U3vStreamBuilder`.

**`Discovery`, `DeviceOps`, `StreamSource`** (`viva-service`) — the one
service implementation both transports run. A transport supplies a
`Discovery` (find cameras, name each one's announce, connect); its device is
`DeviceOps` for features — a `SharedCamera` that runs every call on the
blocking pool — and a `StreamSource` for frames, whose `BlockStream` the
acquisition queryable reads. `run_discovery` serves each new camera, re-announces
the known ones and publishes a lost one disconnected; `run_service` wraps it in
a Zenoh session and CTRL+C. GigE Vision's `DeviceHandle` also owns
reconnection with exponential backoff, which needs no keepalive coordination:
the replacement camera brings its own, and the old one retires when the swap
drops the camera that owned it. `viva-service-u3v` is a USB `Discovery` and a
`U3vDeviceHandle<T: UsbTransfer>`, so the same code runs against
`FakeU3vTransport` (`--fake`) and `NusbTransfer`.

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
the image and its chunks by ID (`viva_gige::parse_chunks`, read from the
end; `viva_u3v::parse_chunks`, the same layout with little-endian tags); their
values are read through the nodemap's chunk ports, never decoded
here.

**Service path** — camera → `viva-service`/`viva-service-u3v` (one
`run_discovery` over the transport's `Discovery`) → Zenoh (types from
`viva-zenoh-api`) → studio or other subscribers. The service owns discovery
announcements, XML serving, node read/write, and frame publication; a frame's
format travels as its PFNC name in `image/meta` and as a one-byte code in the
frame header, both `viva_pfnc::PixelFormat` in Rust.

## Error handling policy

Each crate defines its own typed error enum, and an error carries its cause
([ADR-0030](adrs/adr0030-errors-and-public-surface.md)):

- A failure from another crate or from `std` is the variant's `source`, never
  `format!`ted into a `String`. A cause whose type is not ours to expose — a
  transport behind `RegisterIo`, the XML reader, the USB library — is a
  `Box<dyn Error + Send + Sync>` that can still be downcast: a GenApi
  `GenApiError::Io` holds the `GigeError`, the `U3vError` or the facade's
  `WriteRefused` that refused the register access.
- A refusal of our own names what was refused in typed fields
  (`Access { name, refused }`, `MaskedWriteUnreadable { name, address }`); a
  fixed description is a `&'static str`.
- The facade says what it was doing: `GenicamError::Gige { during, source }`.
- `Display` prints the cause too (`"{during}: {source}"`), because Python,
  Studio, the services and the logs print an error with `{e}` and nothing
  else.
- A status code decodes with its transport's table: `GigeStatus`, `U3vStatus`
  over the shared `viva_gencp::StatusCode`
  ([ADR-0020](adrs/adr0020-per-transport-status-codes.md)).
- Every public error type is `#[non_exhaustive]`; where else the attribute
  goes is ADR-0030's rule.

Panics must never cross a public API in response to remote input — a
malformed camera XML or a hostile packet is an `Err`, not a crash.

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
  happens to send. Fakes import the transport crates' vocabulary — register
  addresses, opcodes, status codes — and nothing else: they encode and decode
  every packet themselves ([ADR-0030](adrs/adr0030-errors-and-public-surface.md)).
- Wire formats are pinned by spec-derived golden byte fixtures, asserted
  independently of the client parser — a round trip through our own code proves
  only that the two agree.
- Tests assert payload sizes and content, not just headers and status codes,
  and identity assertions check values rather than presence.
- A protocol feature with no fake implementation is not considered implemented.
- Predicates and test fixtures wire onto real SFNC features; no synthetic
  `Test*` nodes in fake-camera XML.
- An independent consumer reads the fakes too: the GenTL job drives
  `viva-fake-gige` through Harvesters and EMVA's reference GenApi
  (ADR-0031), which rejected the fake's XML that our own parser accepted.

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
