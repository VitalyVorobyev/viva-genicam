# Roadmap

The order of the work ahead. **This file only looks forward**: shipped work lives in the [changelog](../CHANGELOG.md), tasks and their evidence in [backlog.md](backlog.md). The public, user-facing list of gaps is [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191); this file orders the work that closes them. Delete a milestone when it merges.

**Ordering principle.** Priority is argued from measurement (a corpus count, a wire capture, a user report), not intuition ([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)): wrong results first, then streaming reliability, breadth real cameras use, usability, reach, and USB3 Vision last. The [evidence hierarchy](../CLAUDE.md#evidence-hierarchy) can promote any row when a user with hardware appears. Corpus counts live only in the backlog, and a row no milestone names stays there unscheduled.

**One release.** The milestones below ship together as **0.9.0**, at the end of the sequence: one PR per milestone, merged in this order. R, S, C, L and U are derived from the specifications and arithmetic, so the release notes say what was verified only on fakes.

## R — One GVSP receive engine

**Goal**: one receive thread and one assembler on every OS, replacing the two in the library and the four in tools and examples. Frames time out from their last packet, extended IDs land where they belong, a `Frame` says whether it is complete, and the counters account for every block the camera sent.
**Rows**: SR-13, SR-14, SR-15, SR-16, SR-17, SR-18, SR-07, SR-09, SR-12, TC-05, API-01, API-13, API-17, CI-09, and the stream side of PY-01.

## S — Packet resend and stream-channel setup

**Goal**: a lost packet is requested again, in the specification's layout, when the camera supports it. Stream setup stops overwriting the packet delay, binds a port of its own and accepts packets only from the camera.
**Rows**: SR-04, SR-03, SR-19, SR-21, SR-22, TC-11.

## C — Chunks and events through GenApi ports

**Goal**: chunk and event features read through their own ports, keyed by `ChunkID` and `<EventID>`, in the specification's layouts.
**Rows**: GA-12, GA-13, GA-39, TC-06, TC-21.

## L — GigE session lifecycle and discovery

**Goal**: closing a camera releases it, a camera another application controls can be watched, and losing control is reported. Discovery finds a camera on the wrong subnet and says so.
**Rows**: TC-28, TC-29, TC-30, TC-31, TC-32, SR-20, SR-08, API-18, TC-25, TC-26.

## P — Pixel formats and image I/O

**Goal**: the packed formats cameras send, one conversion module shared by the library, Python and Studio, and images saved at full bit depth.
**Rows**: PX-01, PX-02, PX-03.

## F — Facade: typed and type-gated `Camera`, SFNC helpers, blocking API

**Goal**: common tasks without knowing each vendor's feature names or running an async runtime; GigE-only methods only on GigE cameras.
**Rows**: API-14, API-15, API-16, API-05, API-06, API-08, API-09.

## V — Configuration: persistence and the field-engineer CLI

**Goal**: save a camera's configuration to a file and restore it, and do from `viva-camctl` what a field engineer does with a vendor tool.
**Rows**: GA-14, CL-01.

## K — Caching, polling and refresh

**Goal**: values the camera changes on its own read fresh, and `Camera`, Python and Studio can ask what changed.
**Rows**: GA-05, GA-40.

## Y — Python parity and threads

**Goal**: the Python package offers what the Rust facade does, from any thread, with NumPy arrays of the right shape and type; the Windows wheel is tested.
**Rows**: PY-01, PY-02, PY-03, PY-04, CI-04.

## X — Errors, public surfaces, services

**Goal**: freeze the surface the GenTL producer will wrap: error source chains, per-transport status codes, curated re-exports, one service for both transports.
**Rows**: API-02, API-03, API-04, API-07, API-10, GA-30, TC-10, TC-16, SVC-04.

## T — GenTL producer

**Goal**: a `.cti` through which GenTL consumers use viva-genicam, tested in CI by a consumer we did not write.
**Rows**: GT-01.

## U — USB3 Vision robustness

**Goal**: several transfers in flight, recovery from transfer errors, hot-unplug, and parity with GigE. Last, because no USB3 Vision camera has run any of it.
**Rows**: UV-01, UV-02, UV-03, API-11; TC-27 stays blocked.

## Waiting on hardware

- **GA-44**: whether cameras clear a command register once the command is done, which `IsDone` relies on.
- **TC-12**: the width of `PENDING_ACK`'s timeout field; needs a capture, once L enables `PENDING_ACK` (TC-30).
- **TC-24**: how a device answers a heartbeat during a `PENDING_ACK`ed command; the same capture settles it.
- **TC-27**: USB3 Vision has never met a camera; needs `viva-camctl report --usb 0` from one.
- **DC-02**: `Coord3D_*` on the Zenoh wire; a wire-format change, made only together with DC-03.
- **DC-03**: `Scan3d*` scale, offset and invalid value; one capture ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93)) is the only evidence of what a 3D device sends.
- **DC-04**: a fake that emits `Coord3D_*` frames, built from that one capture; behind GA-09.
- **DC-05**: event-vision blocks in Python; their format is known only from [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)'s camera. R removes the Windows receiver its test targets.
- **DC-06**: `stream --raw-out` edge cases; only event-vision streams write raw output, so it goes with DC-05.

## Studio

Studio is the library's reference viewer ([ADR-0024](adrs/adr0024-studio-reference-viewer.md)); each milestone updates it when a type it consumes changes.

- **Foundations**: ST-45, ST-46, ST-47.
- **Viewer parity**: ST-39.
- **Distribution**: ST-07.
- **Later**: ST-08, ST-09, ST-10, ST-11, ST-12, ST-13.

## Not planned for 0.9.0

- CoaXPress and Camera Link: reachable only through a vendor's GenTL producer, which we do not consume ([ADR-0011](adrs/adr0011-pure-rust-genicam-stack.md)).
- PTP (IEEE 1588) configuration helpers (TC-35).
- FileAccess helpers and firmware update (TC-34).
- GenApi XML from `http:`/`file:` URLs, the manifest table and a host-side cache (TC-33).
- More than one camera at a time in Studio (ST-42).
- Minor GenApi metadata: display notation, aliases, deprecation (GA-41).
