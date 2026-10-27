# Roadmap

The order of the work ahead. **This file only looks forward**: shipped work lives in the [changelog](../CHANGELOG.md), tasks and their evidence in [backlog.md](backlog.md). The public, user-facing list of gaps is [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191); this file orders the work that closes them. Remove an ID here when its row ships.

**Ordering principle.** Priority is argued from measurement (a corpus count, a wire capture, a user report), not intuition ([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)): wrong results first, then streaming reliability, breadth real cameras use, usability, reach, and USB3 Vision last. The [evidence hierarchy](../CLAUDE.md#evidence-hierarchy) can promote any row when a user with hardware appears. Corpus counts live only in the backlog, and a row this file does not name stays there unscheduled.

## Hardware validation

The Manta and [QUISS six-sensor bench](hardware-rtvision-2026-10-26.md) run the [hardware test plan](hardware-test-plan.md). Hardware availability promotes these rows:

- **Found on hardware**: TC-41; **SR-28**, complete-frame delivery under high-rate acquisition. Resolve measured delivery drops before another endurance run.
- **Control channel**: TC-38.
- **GenApi**: TC-39, API-19.
- **Local validation**: run the opt-in hardware suite while a camera is available. CI-22 is deferred: no self-hosted runner or hardware workflow is planned.
- **Endurance**: SR-27, decided by the plan's area L and the [acquisition profile](performance-2026-10-26.md). Repeat endurance with resource and restoration evidence when hardware is available; a buffer pool needs a measured bottleneck.
- **Evidence**: the [tested-camera page](../book/src/tested-cameras.md) records completed hardware runs.

## Waiting on hardware

- **TC-12**: the width of `PENDING_ACK`'s timeout field; a capture of a camera that sends one settles it.
- **TC-24**: how a device answers a heartbeat during a `PENDING_ACK`ed command; the same capture settles it.
- **TC-27**: USB3 Vision on Linux, a SuperSpeed link, another vendor's camera, and through an unplug mid-stream; one camera has run it, on macOS over USB 2 and on Windows.
- **UV-05**: which transfer errors a camera recovers from; a `-vv` log of a stream that hit one settles it.
- **UV-06**: an event that asks for an acknowledgement the USB3 Vision event interface cannot send.
- **TC-21**: whether #112's `BAD_ALIGNMENT` reads were event-port registers, as their names and the corpus suggest; needs the reporter's confirmation or XML.
- **TC-37**: chunk features read through a chunk port, and event ports, have met no camera; the Manta G-125B confirmed the chunk and event layouts and the event timing but declares neither port.
- **DC-02**: `Coord3D_*` on the Zenoh wire; a wire-format change, made only together with DC-03.
- **DC-03**: `Scan3d*` scale, offset and invalid value; one capture ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93)) is the only evidence of what a 3D device sends.
- **DC-04**: a fake that emits `Coord3D_*` frames, built from that one capture.
- **DC-05**: event-vision blocks in Python; their format is known only from [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)'s camera.
- **SR-23**: the receive engine's restart, late-window and oversized-packet rules; a camera whose block ids restart at 1, or whose packet size the host misjudges mid-stream, settles them (the Manta G-125B's ids run on).
- **SR-25**: the standard-id resend request's first word on a stream channel other than 0, and other vendors' answers to a resend they cannot serve.
- **DC-06**: `stream --raw-out` edge cases; only event-vision streams write raw output, so it goes with DC-05.
- **PX-05**: how a camera that declares line padding counts a packing group its line ends inside (an odd width in `Mono12p`); needs a capture from such a camera.
- **TC-38**: whether cameras other than the Manta G-125B refuse a non-holder's claim and writes while nobody holds control, and whether the keepalive should write CCP to see a takeover at once.
- **TC-39**: whether a widened WRITEMEM (a 1-byte register) is accepted; a `-vv` log of one.
- **API-19**: whether a camera declares an exposure name it does not implement, and what SVS's binning `On` does; a report from either camera settles it.
- **GA-46**: whether vendor tools' configuration files are the layout ADR-0028 writes; a user's saved file settles it.
- **TC-40**: whether Linux and Windows deliver a broadcast discovery acknowledgement to the discovery socket.

## Studio

Studio is the library's reference viewer ([ADR-0024](adrs/adr0024-studio-reference-viewer.md)); a change to a type it consumes updates it in the same PR.

- **Found on the bench**: **ST-61**, native confirmation of the implemented remote control-panel refresh after replacement; ST-59, the earlier hand-assembled macOS bundle failure; the proper-bundle [native follow-up](hardware-2026-10-25.md#native-studio-follow-up-2026-10-26) passes.
- **Viewer parity**: ST-51, ST-52, ST-56, ST-49; hardware confirmation of corrected preview cadence ST-60. Native sample inspection and PNG snapshots use an explicit frozen capture ([ADR-0034](adrs/adr0034-native-frame-analysis.md)).
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
