# Roadmap

The order of the work ahead. **This file only looks forward**: shipped work lives in the [changelog](../CHANGELOG.md), tasks and their evidence in [backlog.md](backlog.md). The public, user-facing list of gaps is [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191); this file orders the work that closes them. Delete a milestone when it merges.

**Ordering principle.** Priority is argued from measurement (a corpus count, a wire capture, a user report), not intuition ([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)): wrong results first, then streaming reliability, breadth real cameras use, usability, reach, and USB3 Vision last. The [evidence hierarchy](../CLAUDE.md#evidence-hierarchy) can promote any row when a user with hardware appears. Corpus counts live only in the backlog, and a row no milestone names stays there unscheduled.

**One release.** The milestones below ship together as **0.9.0**, at the end of the sequence: one PR per milestone, merged in this order. U, like the receive engine, packet resend, the chunk and event ports and the GigE session lifecycle already on `main`, is derived from the specifications and arithmetic, so the release notes say what was verified only on fakes.

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
- **TC-12**: the width of `PENDING_ACK`'s timeout field; needs a capture, now possible: `PENDING_ACK` is enabled where the camera offers it.
- **TC-24**: how a device answers a heartbeat during a `PENDING_ACK`ed command; the same capture settles it.
- **TC-27**: USB3 Vision has never met a camera; needs `viva-camctl report --usb 0` from one.
- **TC-21**: whether #112's `BAD_ALIGNMENT` reads were event-port registers, as their names and the corpus suggest; needs the reporter's confirmation or XML.
- **TC-37**: the chunk and event layouts (ADR-0026) have met no camera; one capture with chunk mode on settles them.
- **DC-02**: `Coord3D_*` on the Zenoh wire; a wire-format change, made only together with DC-03.
- **DC-03**: `Scan3d*` scale, offset and invalid value; one capture ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93)) is the only evidence of what a 3D device sends.
- **DC-04**: a fake that emits `Coord3D_*` frames, built from that one capture; behind GA-09.
- **DC-05**: event-vision blocks in Python; their format is known only from [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)'s camera.
- **SR-23**: the receive engine's block-id and packet heuristics (late window, restart on block 1, refusing an oversized data packet); a capture through an AcquisitionStop/Start settles them.
- **SR-24**: whether cameras accept a packet resend request sent from the stream socket rather than the control channel's port; a lossy stream's `-vv` log, or a capture, settles it.
- **SR-25**: how a camera answers a resend it cannot serve (which status, per packet or per range, or silence), and whether resent packets carry status `0x0100`.
- **DC-06**: `stream --raw-out` edge cases; only event-vision streams write raw output, so it goes with DC-05.
- **PX-05**: line or image padding when a width ends inside a packing group (an odd width in `Mono12p`) and no padding is declared; needs a capture from such a camera.
- **TC-38**: whether a CCP read tells an application another took control, and whether devices accept writes while nobody holds it; a capture of two applications on one camera.
- **TC-39**: whether a widened WRITEMEM (a 1-byte register) is accepted; a `-vv` log of one.
- **API-19**: whether a camera declares an exposure name it does not implement, and what SVS's binning `On` does; a report from either camera settles it.
- **GA-46**: whether vendor tools' configuration files are the layout ADR-0028 writes; a user's saved file settles it.
- **GA-47**: whether a camera reads back a `WriteAround` register's new value at once, or the old one until it applies the write; a `set` then `get` with `-vv` settles it.

## Studio

Studio is the library's reference viewer ([ADR-0024](adrs/adr0024-studio-reference-viewer.md)); each milestone updates it when a type it consumes changes.

- **Foundations**: ST-45, ST-46, ST-47.
- **Viewer parity**: ST-39.
- **Distribution**: ST-07.
- **Later**: ST-08, ST-09, ST-10, ST-11, ST-12, ST-13, ST-49, ST-50.

## Not planned for 0.9.0

- CoaXPress and Camera Link: reachable only through a vendor's GenTL producer, which we do not consume ([ADR-0011](adrs/adr0011-pure-rust-genicam-stack.md)).
- PTP (IEEE 1588) configuration helpers (TC-35).
- FileAccess helpers and firmware update (TC-34).
- GenApi XML from `http:`/`file:` URLs, the manifest table and a host-side cache (TC-33).
- More than one camera at a time in Studio (ST-42).
- Minor GenApi metadata: display notation, aliases, deprecation (GA-41).
