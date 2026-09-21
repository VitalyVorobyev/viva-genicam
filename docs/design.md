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
  it rather than because we planned the window. The [roadmap](roadmap.md) names
  the release in flight and what gates it.

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
  pixel-format codes and layouts.
- `viva-sfnc` — Standard Feature Naming Convention constants.
- `viva-zenoh-api` — the message payloads and topic names the services
  publish. This is the contract, not the transport: despite the name it does
  not depend on the `zenoh` crate, so a client can share the message format
  without linking a broker, and it compiles everywhere including wasm.

**Test crates (not published):**

- `viva-fake-gige` / `viva-fake-u3v` — in-process fake cameras, the primary
  test vehicle (see Testing strategy below).

The `studio/` GUI workspace (Viva Studio, a Tauri desktop app) lives in this
repository as a second Cargo workspace (ADR-0017); it consumes
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

- `GigeRegisterIo` — async-to-sync adapter over `GigeDevice` using
  `block_in_place` + `block_on`; safe to call from both async and sync
  contexts. Also **owns the control-channel keepalive**: constructing one
  spawns a task that reads `GevHeartbeatTimeout` and refreshes the device's
  timer at a quarter of it, and dropping one retires that task. Holding the
  transport is therefore sufficient to hold control privilege — nothing above
  this layer runs a heartbeat (SR-05; three consumers used to).
- `MockIo` — in-memory register map for tests.
- `NullIo` — no-op backend for offline XML browsing (studio, wasm).

**`NodeMap`** (`viva-genapi`) — parsed from GenICam XML, stores nodes by name,
tracks the dependency graph for cache invalidation. Supports `pValue`
delegation: Integer/Float/Enum/Boolean/Command nodes can delegate to `IntReg`
or other backing nodes. The `Node` enum covers Integer, Float, Enum, Boolean,
Command, Category, SwissKnife, Converter, IntConverter, String. Introspection
API (`node_names()`, `dependents()`, `categories()`, `kind_name()`,
`access_mode()`) serves external consumers such as the studio.

**`GigeDevice`** (`viva-gige`) — async UDP wrapper for GVCP discovery/control
and GVSP streaming. Uses the proper GVCP wire format (0x42 key byte, 4-byte
addresses).

**`FrameStream` / `U3vFrameStream`** (`viva-genicam`) — async frame iterators.
The GigE side reassembles GVSP packets into frames; the U3V side wraps blocking
USB bulk reads via `spawn_blocking` + an mpsc channel, created through
`U3vStreamBuilder` or `U3vFrameStream::start()`.

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
→ transport reassembles leader/payload/trailer into a complete buffer → `Frame`
with pixel-format metadata from `viva-pfnc` → application via
`FrameStream`/`U3vFrameStream`.

**Service path** — camera → `viva-service`/`viva-service-u3v` (`DeviceHandle`)
→ Zenoh (types from `viva-zenoh-api`) → studio or other subscribers. The
service owns discovery announcements, XML serving, node read/write, and frame
publication.

## Error handling policy

Each crate defines its own typed error enum; errors crossing crate boundaries
carry their source (`#[source]` chains) rather than flattening into strings.
Existing `Transport(String)`-style variants are acknowledged debt slated for
the 0.4.0 consolidation ([roadmap](roadmap.md) Phase 5). Panics must never
cross a public API in response to remote input — a malformed camera XML or a
hostile packet is an `Err`, not a crash.

## Testing strategy

In-process fake cameras (`viva-fake-gige`, `viva-fake-u3v`) are the primary
strategy: no hardware, CI-friendly, deterministic, and fast enough to run the
full integration suite on every push
(see [ADR-0013](adrs/adr0013-fake-camera-first-testing.md)).

**Realism policy** (hard-learned): a fake must implement the *standard's*
semantics, not mirror the implementation's assumptions. This has now failed
three times, each time with producer and consumer agreeing with each other and
jointly disagreeing with the standard, so every test passed:

1. The fake's GVSP sender and our receiver shared the same wrong SCPS
   interpretation (both ignored the 36-byte IP+UDP+GVSP overhead).
2. The fake accepted unaligned READMEM that real Hikrobot hardware rejects.
3. The fake emitted the Discovery ACK MAC at the same wrong offset the parser
   read it from (#57).

[ADR-0019](adrs/adr0019-transport-conformance-and-spec-derived-fakes.md) turns
the policy into an enforced one. Consequences:

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
   pixel formats owned by `viva-pfnc`.
4. **YAGNI** — no speculative scaffolding; dead code gets deleted or wired up
   (the unused GVSP resend machinery is the cautionary tale).
5. **Spec-conformance over self-consistency** — tests validate against the
   standard, not against our own second implementation of the same mistake.
6. **Errors carry sources; panics never cross a public API on remote input.**

When a change conflicts with a tenet, write or update an ADR in `docs/adrs/`
rather than silently deviating.
