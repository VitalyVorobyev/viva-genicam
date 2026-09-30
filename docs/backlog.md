# Backlog

Immediate, actionable tasks. Mid-term direction lives in
[roadmap.md](roadmap.md); shipped work is recorded in
[CHANGELOG.md](../CHANGELOG.md).

**Legend**

- Priority: **P0** (do next) / **P1** (soon) / **P2** (when convenient)
- Size: **S** (hours) / **M** (a day or two) / **L** (several days) /
  **XL** (a week+)
- Status: `planned` / `in-progress` / `blocked`

Every row carries its evidence — a corpus count, a code symbol, or an issue
number — so priority can be argued from data rather than from intuition
([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)).

**This file only lists open work.** Delete a row in the PR that ships it;
[CHANGELOG.md](../CHANGELOG.md) records what shipped and why.

Filing rules:

- **Do not propose downgrading a log level before the warning's cause is known** — that deletes the evidence.
- **A row filed from fake-camera output alone is a hypothesis** until real hardware shows the same thing.

## TC — Transport conformance (roadmap Phase 1, ADR-0019)

The GVCP/GVSP audit ADR-0018 never reached. Opcodes cross-checked against
`../aravis/src/arvgvcpprivate.h`.

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| TC-05 | Accept the payload types cameras actually send | P1 | M | planned | `parse_leader` (`crates/viva-gige/src/gvsp.rs`) reads the payload type as `get_u16() as u8`, so Image Extended Chunk `0x4001` is stored as plain image `0x01` and the chunk bit is lost; types whose low byte is not `0x01` (`0x0002` raw data, `0x0004` file) and packet format `0x04` (single-packet block) are rejected. Two vendors send `0x4001` on hardware: #70's JAI and a scanCONTROL 850050 ([#93 capture](https://github.com/VitalyVorobyev/viva-genicam/pull/93#issuecomment-5162882332)). **Scope**: carry the full `u16` through `GvspPacket::Leader` and accept those types |
| TC-06 | Chunk trailer layout is self-consistent, not conformant | P1 | M | planned | `viva_gige::gvsp::parse_chunks` decodes front-to-back `[id][reserved][len][data]` big-endian, while `decode_known` in `crates/viva-genicam/src/chunks.rs` reads values little-endian. The spec layout is backward-scanned `[data][id][len]` tuples. The fake emits our layout, so the round trip proves nothing |
| TC-10 | `viva-gencp::encode_cmd` is dead *and* divergent | P2 | S | planned | It writes a different header shape (no `0x42` key byte) than `viva-gige` actually uses, and has no callers outside its own tests and README (`crates/viva-gencp/src/lib.rs`) |
| TC-11 | `mtu()` returns 1500 on macOS | P2 | M | planned | `viva_gige::nic::mtu` reads `/sys/class/net` on Linux and `GetIfEntry2` on Windows; every other platform falls through to a hardcoded 1500, so jumbo frames can never be selected on macOS |
| TC-12 | Settle the GVCP `PENDING_ACK` field width against hardware | P2 | S | planned | GigE Vision 1.2 §18.5 defines 2 reserved bytes + a `u16` `time_to_completion`; aravis reads all four payload bytes as a big-endian `u32`. The readings agree only while `reserved` is zero. `parse_pending_ack` (`crates/viva-gige/src/gvcp.rs`) implements the spec `u16`. A capture from a camera that actually sends one decides it — and if a real device puts the timeout in the full `u32`, that device wins |
| TC-16 | Per-transport status-code types | P2 | M | planned | GVCP and GenCP share `0x0000`, `0x8001`-`0x8007` and `0x8FFF` but conflict at `0x800B` (`NO_MSG` vs `MSG_TIMEOUT`), and each owns codes the other lacks. A transport-specific code decodes as `StatusCode::Unknown(raw)` — reportable but unnamed; no current path decodes `0x800B`. **Scope**: [ADR-0020](adrs/adr0020-per-transport-status-codes.md) (Proposed) points 1-2, a shared core plus per-transport types. Breaking: land it in the next breaking window with API-02/API-03 |
| TC-18 | Stale-ack absorption is bounded by a count, not a deadline | P2 | S | planned | Promised to [#91](https://github.com/VitalyVorobyev/viva-genicam/pull/91)'s author. `GigeDevice::transact_with_retry` absorbs up to `MAX_RETRIES` mismatched acks and each `recv_absorbing_pending` call restarts `CONTROL_TIMEOUT`, so trickling stale acks can stretch one attempt to 4 × 500 ms. aravis bounds the window by wall clock (`arvgvdevice.c`, `timeout_stop_ms`). Revisit if it shows on hardware; a scanCONTROL 850050 stress test showed none ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93#issuecomment-5162882332)) |
| TC-21 | `BAD_ALIGNMENT` on a real camera's event registers | **P1** | M | planned | **Seen on hardware** ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), Studio log): 62 reads of `pEventStream*Reg` on a Vieworks FS3200T return `BAD_ALIGNMENT (0x8005)`. `GigeDevice::read_mem` rounds the *count* to a multiple of four ([#35](https://github.com/VitalyVorobyev/viva-genicam/issues/35)); nothing checks the *address*. READREG dispatch does not help: `single_register_address` only sends aligned 4-byte accesses. **Next step**: get the reporter's XML to see whether these are `<IntReg>` at unaligned addresses |

## DX — Diagnostics loop (roadmap Phase 2)

Every fix so far was diagnosed from a user-supplied artifact, and the library
offers no supported way to produce one.

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| DX-05 | Surface skipped nodes in Python and Studio | P1 | S | planned | `NodeMap::skipped()` already carries them out through `viva-camctl` (0.4.0). `viva-pygenicam` and Studio still cannot ask: a Studio user seeing a missing feature has no way to learn it was dropped rather than absent |
| DX-06 | `viva-camctl report` for USB3 Vision | P2 | M | planned | The bundle is GigE-only — it opens a GVCP control channel and dumps GigE bootstrap registers. A U3V reporter gets nothing equivalent, though `viva-camctl list-usb` exists |

## SR — Streaming reliability (roadmap Phase 3)

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| SR-03 | Per-stream ephemeral ports + `source_filter` enforcement | P0 | M | planned | `StreamConfig::source_filter` is set in `StreamBuilder::build` (`crates/viva-genicam/src/stream.rs`) and never read, because `PacketSource::recv` discards the source address. The value stored is the stream destination (the host's own address), not the camera's |
| SR-04 | Wire packet resend end-to-end, or delete it | P1 | L | planned | `ResendPlanner` (`crates/viva-gige/src/gvsp.rs`), `GigeDevice::request_resend` (`gvcp.rs`) and `StreamConfig::resend_enabled` have zero production callers. Pick one; the telemetry side is SR-07 |
| SR-07 | Honest streaming telemetry | P1 | M | planned | `StreamStatsAccumulator::record_resend`, `record_resend_ranges`, `record_late_frame` and `record_pool_exhaustion` (`crates/viva-gige/src/stats.rs`) have no production caller (only `examples/stats.rs`, which fabricates them), so `resends=0` means "not implemented", not "healthy". `record_backpressure_drop` fires only in `windows_frame_receiver` and the dead `FrameQueue`. GVSP parse errors in `FrameStream` are dropped at `trace` and counted nowhere. Until fixed, `drops` is the only reliability figure worth reading |
| SR-08 | IGMP leave on multicast stream teardown | P2 | S | planned | `nic::bind_multicast` / `nic::join_multicast` (`crates/viva-gige/src/nic.rs`) join the group; nothing ever leaves it |
| SR-09 | Delete or wire the unused reassembly stack | P2 | M | planned | Part of API-01, which owns the scope: `Reassembler`, `FrameAssembly`, `FrameQueue`, `BufferPool` and `GigeDevice::negotiate_stream` have no callers |
| SR-12 | Per-packet `HashSet` insert on the receive hot path | P2 | S | planned | `FrameAssemblyState::received_packet_ids` (`crates/viva-genicam/src/stream.rs`) is a `HashSet<u32>` taking one insert per datagram; at a 1500-byte packet size a 3.1 MB frame costs ~2 100 hashes. A grow-only bitmap or a max-ID watermark does the same job allocation-free. The data structure is chosen in API-01; this row keeps it from being lost if API-01 slips |

## GA — GenApi conformance, round 2 (roadmap Phase 4)

Counts drift as the corpus grows: recount with a whole-element match
(`grep -c` undercounts the single-line FLIR/PGR documents) and don't quote a
count from this table.

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| GA-03 | `<pSelected>` is parsed with the direction inverted | P1 | M | planned | 1 534 occurrences in 31 docs. The standard puts it on the *selector* naming the *selected* feature (`fixtures/vendor-xml/Basler_acA1600_20gm.xml:213`); `handle_p_selected_start` (`crates/viva-genapi-xml/src/parsers/mod.rs`) treats the text as this node's selector, registering invalidation edges backwards |
| GA-04 | Activate `pMin`/`pMax` in range checks | P1 | M | planned | 1 288 / 911 occurrences, 35 / 34 docs. Parsed (Integer only), stored on `IntegerNode`, registered as dependencies, and read for formula `.Min`/`.Max` since GA-29 — but `NodeMap::set_integer`'s range check still uses the static `node.min`/`node.max`, so a dynamic limit is ignored on write |
| GA-05 | Honour `Cachable` and `PollingTime` | P1 | L | planned | 2 735 / 32 docs and 327 / 28 docs, both unparsed, so every readable node is cached until a dependency is written. `<pInvalidator>` (GA-24) covers values the XML links to a write; what remains is a value the camera changes on its own. Example: FLIR's `ExposureTime_Val` is `<Cachable>NoCache</Cachable>` with `<PollingTime>500</PollingTime>`, so under auto exposure a cached read is stale. By value: `WriteThrough` 2 026, `NoCache` 883, `WriteAround` 344 |
| GA-07 | Honour `<Slope>` on Converter min/max propagation | P1 | S | planned | 632 occurrences in 29 of 35 documents. A decreasing converter inverts min and max; we propagate neither |
| GA-09 | `<pLength>` on `<Register>` | P1 | M | planned | Plain-`<Length>` registers shipped in 0.4.0 (`NodeMap::get_register`/`set_register`, `register_address`). Remaining: `<pLength>`, a length resolved from another node at runtime — 22 declarations, recounted 2026-09-30: Hikrobot 11, Basler 2+2, seven documents with one each. Meanwhile the skip error must name `<pLength>`, and `EXPECTED_SKIP_REASONS` stays keyed by (tag, error substring) so no other `<Register>` failure hides behind it. `<pPort>` on a non-device port stays refused (GA-12) |
| GA-10 | `exec_command`'s direct-address path skips the write guards | P2 | S | planned | `NodeMap::exec_command` routes a `<pValue>` command through `set_integer` (guarded by `ensure_writable_now`) but writes a bare-`<Address>` command with raw `io.write` — no access-mode, availability or `pIsLocked` check. P2: all 432 `<Command>` nodes in the 35 vendor documents use `<pValue>`. Yet `viva-fake-gige` has three direct-address commands and one `pValue` one (`UserSetLoad`), so tests mostly exercise the path no camera uses; switch them when fixing this |
| GA-11 | Corpus test asserts no values, so a wrong one still passes | P1 | M | in-progress | `crates/viva-genapi/tests/vendor_corpus.rs` evaluates every document against `NullIo` and a test-local `PatternIo` (bytes `FF FE FD …`, top bit set in either byte order), where `GenApiError::Parse` is an engine defect ([ADR-0022](adrs/adr0022-integer-register-decoding.md)). **Remaining**: it asserts only that nothing errors, so a byte-swapped register still passes; add per-document value expectations. Keep `PatternIo` test-local — `NullIo`'s zero contract is public API (Studio, wasm) |
| GA-12 | GenApi chunk adapter | P2 | L | planned | Replaces the hardcoded 4-entry `KNOWN_CHUNKS` table in `crates/viva-genicam/src/chunks.rs`. `ChunkID` appears 164 times in 14 docs and is ignored; `RegisterIo` has no port abstraction to address a chunk port with |
| GA-13 | Support negative `<Address>` (chunk-relative offsets) | P2 | M | planned | Confirmed: `Baumer_HXG20` `ChunkImageLength`; the sole `EXPECTED_SKIPS` entry in both corpus tests. Needs a signed `AddressTerm::Fixed` plus a chunk base |
| GA-14 | `Streamable` — no persistence/save-load feature exists | P2 | L | planned | 1 700 occurrences, 18 docs. Feature-set save/restore is a standard GenApi capability we do not offer |
| GA-15 | Fall back to lossy decoding for non-UTF-8 GenApi XML | P2 | S | planned | Unconfirmed. `fetch.rs` `String::from_utf8` is strict, so an ISO-8859-1 document fails the connect outright. **Distinct from the UTF-8 BOM defect** fixed in 0.5.0 ([#122](https://github.com/VitalyVorobyev/viva-genicam/issues/122)), which was valid UTF-8 that parsed into nothing |
| GA-16 | Accept uppercase `0X` hex prefix in `parse_u64`/`parse_i64` | P2 | S | planned | Unconfirmed. Hex digits are already case-insensitive, only the prefix is not. No corpus document uses it |
| GA-17 | Inline `<IntSwissKnife>` as a register address term | P2 | M | planned | Unconfirmed: no corpus document nests one inside a register, but the schema allows it. Today `skip_element` drops the term silently |
| GA-18 | Decide the sign of scaled `<Float>` and `<Enumeration>` payloads | P2 | S | planned | GenICam declares no `<Sign>` for either; 0.3.0 kept the historical signed reading. Worth confirming against hardware before changing |
| GA-23 | `<ImposedAccessMode>` is parsed nowhere | P1 | S | planned | 3 015 occurrences in 31 of 38 documents (2026-09-30), swallowed by `skip_element`. `FLIR_BFS_PGE_31S4C_C` declares it 257 times: `SensorWidth` is `<ImposedAccessMode>RO</ImposedAccessMode>` and we expose it RW. It is a ceiling on access, not a default, so it cannot be folded into `<AccessMode>` handling |
| GA-26 | `AccessMode::parse` maps anything unrecognised to `RW` | P2 | S | planned | `AccessMode::parse` (`crates/viva-genapi-xml/src/lib.rs`) now logs an unknown value, but still defaults it to the most permissive mode; `RO` would be the safe default. Its own test asserts the `RW` fallback |
| GA-30 | `GenApiError::Parse` is a generic conversion bucket | P2 | S | planned | Scope lives in API-03. Tracked here because it is also why the corpus test cannot tell a register-decode failure from a parse failure (GA-11) |
| GA-33 | `<Float>` `<pMin>`/`<pMax>` and `<Integer>` `<pInc>` are dropped by the parser | P2 | M | planned | `parsers/numeric.rs` keeps `<pMin>`/`<pMax>` for integers only and `<pInc>` for nothing. Corpus (regex outside comments): `<Float>` `<pMax>` 390 / `<pMin>` 372 (34 docs each), `<Integer>` `<pInc>` 377 (27 docs). Formula `F.Max`/`I.Inc`, `set_float`'s range check and `viva-service`'s reported ranges read the placeholder (`f64::MAX`, increment 1) — FLIR's `ExposureTime` gets `f64::MIN`/`f64::MAX`, harmless only because the `pValue` branch returns before the range check. Changes `NodeDecl::Float`/`Integer`, which the studio deserializes |

## API — Public-surface consolidation (roadmap Phase 5, breaking)

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| API-01 | Single frame-reassembly implementation | P1 | L | planned | `crates/viva-genicam/src/stream.rs` has two: `FrameAssemblyState` and the Windows-only `WindowsFrameAssembly` ([#72](https://github.com/VitalyVorobyev/viva-genicam/pull/72)), which has no bitmap, resend accounting or out-of-order tolerance — Windows gets weaker reassembly. Unifying drops `#[cfg_attr(windows, allow(dead_code))]` on `FrameAssemblyState` and settles SR-09's unused `viva-gige` stack (`Reassembler`, `FrameAssembly`, `FrameQueue`, `BufferPool`, `GigeDevice::negotiate_stream`, which `StreamBuilder::build` re-implements inline) |
| API-02 | Curated public surfaces | P1 | L | planned | Kill blanket `pub mod` (viva-gige, viva-u3v, viva-service, viva-camctl, viva-fake-gige). Re-export `ConverterNode`, `IntConverterNode`, `StringNode` and `PredicateRefs` from `viva-genapi`'s root — they are payloads of public `Node` variants. **Rule**: never expose `viva_genapi_xml::Addressing` (or `AddressTerm`, `IndexOffset`) through `viva-genapi`, since GA-09 and GA-13 change it; offer name-keyed methods like `NodeMap::register_address` ([#92](https://github.com/VitalyVorobyev/viva-genicam/pull/92)) |
| API-03 | Error source chains (no `String` payloads); `#[non_exhaustive]` policy | P1 | M | planned | Includes GA-30: `GenApiError::Parse` (`crates/viva-genapi/src/error.rs`) is raised by `conversions.rs` for register-decode failures, so a failed device read reads as bad XML. `GenApiError::Access` cannot say which direction was refused and has no test. Follow the shape of `MaskedWriteUnreadable { name, address }` (#135). `thiserror` 2 is already workspace-wide. Adding a variant is breaking — schedule with a minor |
| API-04 | Dedupe viva-service vs viva-service-u3v behind a `StreamSource` trait | P1 | L | planned | ~60% copy-paste today |
| API-05 | Typed accessors on `Camera` | P1 | M | planned | `Camera::get`/`Camera::set` (`crates/viva-genicam/src/lib.rs`) are string-only while `NodeMap` already has `get_integer`/`get_float`/`get_bool`/`get_enum`. Floats round-trip through `to_string`/`parse` |
| API-06 | Type-gate the GigE-only methods on `Camera<T>` | P1 | M | planned | `Camera::configure_events` and `Camera::configure_stream_multicast` (`crates/viva-genicam/src/lib.rs`) write GVCP bootstrap registers but are defined on the generic `Camera<T>`, so they compile — and silently misbehave — against a U3V camera |
| API-07 | Stop exposing node cache internals | P2 | S | planned | `SkNode`, `ConverterNode`, `IntConverterNode`, `StringNode` and `RegisterNode` have `pub cache: RefCell<Option<(_, u64)>>` (`crates/viva-genapi/src/nodes.rs`), publishing the internal generation counter; `IntegerNode`, `FloatNode`, `EnumNode` and `BooleanNode` keep theirs `pub(crate)`. Pick one |
| API-08 | `Camera` get/set inconsistencies | P2 | S | planned | `Camera::get` on a Category returns `Ok("")`; `Camera::set` on a Command executes and ignores the value; Converter/IntConverter are readable but not writable through `Camera` although `NodeMap::set_converter`/`set_int_converter` exist |
| API-09 | Missing `NodeMap`/`Node` accessors | P2 | S | planned | No `Node::unit()`/`min()`/`max()`/`inc()` though the data is stored; `set_string` takes `&self` while every other setter takes `&mut self`; `get_integer` handles `IntConverter` and `SwissKnife` but not `Converter`; `enum_entries` sorts and dedups, discarding the XML order GUIs need |
| API-10 | Fakes import transport-crate register constants; viva-pfnc as single `PixelFormat` authority; workspace lints (`missing_docs`, `unreachable_pub`) | P2 | M | planned | `viva-zenoh-api` defines a second `PixelFormat`, bridged by `pfnc_to_zenoh` in `crates/viva-service/src/pixel_format.rs` |
| API-11 | U3V/GigE facade parity | P2 | L | planned | U3V has no events, chunks (`U3vFrameStream::start` hardcodes `chunks: None`), stats, time sync, or stream params; `U3vStreamBuilder::new` takes a `Camera` where GigE's `StreamBuilder::new` takes a device, and its `build()` is sync where GigE's is async |
| API-13 | `viva-zenoh-api`'s payload types other than `DeviceAnnounce` have no `#[non_exhaustive]` | P1 | S | planned | `DeviceAnnounce` has it plus a `new` constructor ([#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)). `DeviceStatus`, `NodeValueUpdate`, `FeatureState`, `CommandResult`, `AcquisitionStatus`, `ImageMeta` and the request/response structs in `crates/viva-zenoh-api/src/lib.rs` are all-public-field structs, so every field addition is breaking. Each needs a constructor first (the services and studio use struct literals); do it in a release that is already a minor |

## SVC — Services

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| SVC-01 | Announce cadence exceeds Studio's expiry window | P1 | S | planned | GigE re-announces every `discovery_timeout_ms` (2 s) + `discovery_interval_s` (5 s) ≈ 7 s (`crates/viva-service/src/config.rs`); Studio expires at 6 s (`expire_old(6)` in `studio/apps/viva-studio-tauri/src-tauri/src/commands/device.rs`) and `docs/studio/zenoh-api.md` documents 2 s. Devices can flicker |
| SVC-03 | U3V service streaming never configures the SIRM | P1 | M | planned | `U3vDeviceHandle::open_stream` (`crates/viva-service-u3v/src/device.rs`) builds `U3vStream::new` directly with hardcoded 256-byte leader/trailer, bypassing `U3vDevice::open_stream` (`crates/viva-u3v/src/device.rs`), which reads the SIRM and enables streaming |
| SVC-04 | U3V service is single-device with no lifecycle | P2 | M | planned | Takes `devices[0]` once, never re-scans, never publishes disconnect (`crates/viva-service-u3v/src/main.rs`); GigE has a full discovery loop |
| SVC-05 | GigE service clap name is still `genicam-service` | P2 | S | planned | `crates/viva-service/src/config.rs`, pre-rebrand |

## DC — Device classes beyond area-scan

A laser profile scanner (Micro-Epsilon scanCONTROL 850050, [#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93)) is in the corpus and `viva-pfnc` has its `Coord3D_*` codes, but nothing above `viva-pfnc` knows what a `Coord3D_*` frame is. Rows are scheduled only when verified against code or a wire capture, never against our idea of what a 3D camera sends.

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| DC-02 | The Zenoh wire format carries no `Coord3D_*` variants | P2 | M | planned | `viva_zenoh_api::PixelFormat` has one 3D entry, `Coord3D_C16`, which the scanCONTROL does not advertise, and none of the four it does. `#[serde(other)] Unknown` swallows them, so `publish_image_meta` (`crates/viva-service/src/acquisition.rs`) announces `payload_size` as `w * h * 1`. Adding variants changes the one-byte map in `pixel_format_to_u8` (`crates/viva-zenoh-api/src/frame_header.rs`) — a wire-format change; land it with DC-03 |
| DC-03 | No `Scan3d*` SFNC constants, so a `Coord3D` payload cannot be interpreted | P2 | M | planned | `viva-sfnc` has no `Scan3d*` constants and no Rust source reads `Scan3d`, so a `Coord3D_AC16` buffer is raw integers with no metric meaning or invalid-point mask. `MicroEpsilon_scanCONTROL_850050.xml:6104+` declares `Scan3dCoordinateScale`/`Offset`/`Selector`, `Scan3dInvalidDataFlag`/`Value` and `Scan3dOutputMode`, applying scale and offset only to `Coord3D_AC16` (`:6188`, `:6218`); `DeviceScanType` (`:40-61`) offers `Areascan3D`/`Linescan3D`. On the wire `Scan3dInvalidDataValue` is LE `f32` `ff ff ff 7e` = `1.7014117e38`, matching the node ([#93 capture](https://github.com/VitalyVorobyev/viva-genicam/pull/93#issuecomment-5162882332)). The nodes already parse as `Float`/`Enumeration`; add constants and a reader |
| DC-04 | No fake can emit a `Coord3D_*` frame | P2 | M | planned | `viva-fake-gige`'s `build_leader` always sends `PAYLOAD_IMAGE` and `viva-fake-u3v`'s `bytes_per_pixel` knows only RGB8 = 3, else 1, so DC-02/DC-03 have no end-to-end test. **Wire facts** (scanCONTROL 850050, [#93 capture](https://github.com/VitalyVorobyev/viva-genicam/pull/93#issuecomment-5162882332)): leader payload type `0x4001` (image + extended chunk; not GenDC, not multipart), PFNC `0x026000C0`; coordinates little-endian, interleaved per point, 4 / 4 / 8 / 12 bytes for `Coord3D_C32f` / `_AC16` / `_AC32f` / `_ABC32f` (matching `viva_pfnc::PixelFormat::bytes_per_pixel`); in the four decoded `(A, B, C)` tuples `B` is zero for a single profile. **Next step**: build the fake from that capture; P2 behind GA-09 |
| DC-05 | Event-vision blocks: a Python `next_block`, and a test for the Windows receiver | P2 | S | planned | From [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138) / [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139) (Lucid Triton2 EVS, Sony IMX636); the service already publishes event blocks on `genicam/devices/{id}/evs`. Remaining: (1) `viva-pygenicam` exposes only `next_frame`, so Python receives an event block as a 1 × N frame; (2) the `#[cfg(windows)]` receiver branch of `FrameStream` (`device_timestamp` / `set_host_time` on `GenericStreamBlock`) has no test — see CI-04 |
| DC-06 | `viva-camctl stream --raw-out` edge cases | P2 | S | planned | From reviewing [#154](https://github.com/VitalyVorobyev/viva-genicam/pull/154). (1) The file is opened with `append(true)` (`crates/viva-camctl/src/cmd_stream.rs`), so reruns concatenate captures, even of different EVT formats; truncate by default with an explicit `--append`, or refuse a format change. (2) `--save` defaults to 1 (`crates/viva-camctl/src/cli.rs`), so `--raw-out` still writes a `block_0001.*`. (3) The file is created before the first block, so an image stream leaves an empty file |

## CI

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| CI-01 | Per-crate feature matrix (`cargo hack --each-feature`) | P1 | M | planned | `viva-camctl`/`viva-pygenicam`/`viva-service-u3v` all enable `u3v-usb`, so feature unification builds U3V. The gap: `viva-genicam` with its *own* default features (`[]`) is never verified |
| CI-03 | MSRV job | P2 | S | planned | `rust-version = "1.88"` is declared and never checked |
| CI-04 | Windows wheel is built and published but never tested | P2 | M | planned | `python.yml` test matrix is ubuntu + macos-14 only. #57 is a Windows issue |
| CI-05 | Scope release.yml / publish-docs.yml permissions to the jobs that need write | P2 | S | planned | |
| CI-07 | python.yml path filter misses viva-gencp and root Cargo.toml/Cargo.lock | P2 | S | planned | |
| CI-08 | `cargo doc` in ci.yml lacks `--all-features` + `RUSTDOCFLAGS` | P2 | S | planned | |
| CI-09 | Fuzz the packet and XML parsers | P2 | L | planned | Both consume untrusted network input |
| CI-10 | cargo-semver-checks on release tags | P2 | M | planned | |
| CI-17 | Nothing checks links in `docs/` | P2 | S | planned | Links to moved files under `docs/` went stale for weeks unnoticed. `crates/viva-genicam/tests/book_includes.rs` covers mdBook `{{#include}}` anchors only and `ci.yml` greps `mdbook build` for `[ERROR]`; nothing checks ordinary Markdown links, inside or outside `book/` |

## DOC — Documentation

Several of these are *wrong* documentation rather than missing documentation,
which is why they are not all P2.

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| DOC-06 | Book: fill the three crate pages; add the missing ones; write the stubs | P2 | L | planned | No page exists for viva-u3v, viva-genapi-xml, viva-pfnc, viva-sfnc, viva-zenoh-api, the services, viva-camctl or the fakes. USB3 Vision has no tutorial anywhere. `book/src/errors-logging.md` and `book/src/contributing.md` are 24-line stubs |
| DOC-11 | Python `frame.ts_host` is silently wrong for GigE | P1 | S | planned | `build_gige_stream` (`crates/viva-pygenicam/src/stream.rs`) always passes `Some(time_sync)`, but `time_calibrate` is not exposed and `TimeSync::to_host_time` with no origin returns `SystemTime::now()` (`crates/viva-gige/src/time.rs`). Always wall-clock, never device-mapped — worse than `None`, because it looks like data. Fix the behaviour or the docs, not neither |

## ST — Studio

| ID | Task | Priority | Size | Status | Notes |
|----|------|----------|------|--------|-------|
| ST-01 | Modernize studio crates to edition 2024 + workspace-dep inheritance | P2 | M | planned | |
| ST-02 | Retire apps/viva-mock-service in favor of viva-service + viva-fake-gige | P2 | M | planned | |
| ST-03 | Revive studio e2e against in-repo service/fake (drop aravis-from-source) | P1 | L | planned | |
| ST-06 | Release packaging pipeline: DMG (macOS), AppImage (Linux), MSI (Windows) on tag push | P1 | L | planned | Release prep |
| ST-07 | Bundle viva-service binary as Tauri sidecar (auto-start if no external service) | P1 | M | planned | Release prep |
| ST-08 | Frame annotation rendering engine (frame ID/timestamp/FPS burned into BMP stream) | P2 | M | planned | Release prep |
| ST-09 | Annotation toggle in viewer toolbar | P2 | S | planned | Release prep |
| ST-10 | Recording playback engine: load .gsr, play/pause, frame step, speed, seek | P2 | L | planned | Recording & polish |
| ST-11 | Recording export to TIFF stack / uncompressed AVI | P2 | M | planned | Recording & polish; interop with ImageJ, MATLAB |
| ST-12 | Auto-update via Tauri v2 updater plugin | P2 | M | planned | Recording & polish |
| ST-13 | Studio performance benchmarks in CI | P2 | M | planned | Recording & polish; fail on >10% regression |
| ST-14 | Embedded backend has no U3V discovery | P2 | M | planned | `EmbeddedBackend::discover` (`studio/apps/viva-studio-tauri/src-tauri/src/backend/embedded.rs`) returns only what `discover_gige_devices` cached, despite `u3v-usb` being enabled |
| ST-17 | Cover the `StreamInfo` re-send on a mid-stream format change | P2 | S | planned | Promised on [#87](https://github.com/VitalyVorobyev/viva-genicam/pull/87). Studio seeds `StreamInfo` from `PixelFormat` and re-sends when a frame disagrees, but `stream_info_preserves_camera_pixel_format` tests only the constructor; pinning the re-send needs a frame fixture. While there, fold the width/height comparison `start_acquisition` evaluates twice (once for the BMP encoders, once for the re-send) |
| ST-18 | Studio shows enum entries the camera currently rejects | P2 | S | planned | `build_feature_state` (`backend/embedded.rs`) calls `NodeMap::enum_entries`, which returns every entry the XML declares; switch to `NodeMap::available_enum_entries`, which applies the `pIsAvailable` gating. A user is currently offered `PixelFormat` values the camera will refuse |
| ST-22 | Studio cannot set the GVSP packet size | **P1** | S | planned | **Seen on hardware** ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)): on a link whose ceiling is 9198, Studio picked the host MTU and streamed nothing. The embedded backend now calls `StreamBuilder::auto_packet_size()` (MTU + test-packet probe), but a user who has measured the link still cannot override it; `viva-camctl stream --packet-size` can. **Scope**: a connect/acquire field passed to `.packet_size(...)`, and a UI control |
