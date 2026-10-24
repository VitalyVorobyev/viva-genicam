# Roadmap

The order of the work ahead. **This file only looks forward**: shipped work lives in the [changelog](../CHANGELOG.md), tasks and their evidence in [backlog.md](backlog.md). The public, user-facing list of gaps is [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191); this file orders the work that closes them. Remove an ID here when its row ships.

**Ordering principle.** Priority is argued from measurement (a corpus count, a wire capture, a user report), not intuition ([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)): wrong results first, then streaming reliability, breadth real cameras use, usability, reach, and USB3 Vision last. The [evidence hierarchy](../CLAUDE.md#evidence-hierarchy) can promote any row when a user with hardware appears. Corpus counts live only in the backlog, and a row this file does not name stays there unscheduled.

## Hardware validation

One GigE Vision camera on the bench runs the [hardware test plan](hardware-test-plan.md); its areas settle these rows of the next section, in this order:

- **Found on this camera**: CL-03, GA-58, TC-41.
- **Control channel**: TC-38, GT-07.
- **GenApi**: GA-44, GA-47, TC-39, API-19.
- **Streaming**: SR-24, SR-25, SR-23, PX-05.
- **Chunks and events**: TC-37.
- **Repeatable**: CI-21 turns the plan into a command, then CI-22 runs it nightly.
- **Endurance**: SR-27 and API-21, decided by the plan's area L.
- **Surfaces it needs**: CL-02, API-22, PY-06, GT-08; then DOC-19 records what ran.

## Waiting on hardware

- **GA-44**: whether cameras clear a command register once the command is done, which `IsDone` relies on.
- **TC-12**: the width of `PENDING_ACK`'s timeout field; a capture of a camera that sends one settles it.
- **TC-24**: how a device answers a heartbeat during a `PENDING_ACK`ed command; the same capture settles it.
- **TC-27**: USB3 Vision on Linux, a SuperSpeed link, another vendor's camera, and through an unplug mid-stream; one camera has run it, on macOS over USB 2 and on Windows.
- **UV-05**: which transfer errors a camera recovers from; a `-vv` log of a stream that hit one settles it.
- **UV-06**: an event that asks for an acknowledgement the USB3 Vision event interface cannot send.
- **TC-21**: whether #112's `BAD_ALIGNMENT` reads were event-port registers, as their names and the corpus suggest; needs the reporter's confirmation or XML.
- **GT-07**: whether a camera grants exclusive access as the GenTL producer asks for it (CCP `0x1`).
- **TC-37**: GigE Vision's chunk and event layouts (ADR-0026) have met no camera; one capture with chunk mode on settles them.
- **DC-02**: `Coord3D_*` on the Zenoh wire; a wire-format change, made only together with DC-03.
- **DC-03**: `Scan3d*` scale, offset and invalid value; one capture ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93)) is the only evidence of what a 3D device sends.
- **DC-04**: a fake that emits `Coord3D_*` frames, built from that one capture.
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
- **TC-40**: whether Linux and Windows deliver a broadcast discovery acknowledgement to the discovery socket.
- **GA-47**: whether a camera reads back a `WriteAround` register's new value at once, or the old one until it applies the write; a `set` then `get` with `-vv` settles it.

## Studio

Studio is the library's reference viewer ([ADR-0024](adrs/adr0024-studio-reference-viewer.md)); a change to a type it consumes updates it in the same PR.

- **Foundations**: ST-45, ST-46, ST-47.
- **Viewer parity**: ST-39, ST-51, ST-52, ST-56, ST-49.
- **Distribution**: ST-07.
- **Later**: ST-50, ST-53, ST-54, ST-55, ST-57, ST-58, ST-08, ST-09, ST-10, ST-11, ST-12, ST-13.

## Not planned

- CoaXPress and Camera Link: reachable only through a vendor's GenTL producer, which we do not consume ([ADR-0011](adrs/adr0011-pure-rust-genicam-stack.md)).
- The GenTL producer's optional functions, USB3 Vision cameras, events beyond new-buffer, and writing frames in place (GT-02 to GT-05).
- PTP (IEEE 1588) configuration helpers (TC-35).
- FileAccess helpers and firmware update (TC-34).
- GenApi XML from `http:`/`file:` URLs, the manifest table and a host-side cache (TC-33).
- More than one camera at a time in Studio (ST-42).
- Minor GenApi metadata: display notation, aliases, deprecation (GA-41).
