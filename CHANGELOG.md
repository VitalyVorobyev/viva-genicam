# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

### Added

- **Exclusive access.** `Access::Exclusive` opens a GigE Vision camera with exclusive access (CCP `0x1`): while it is held, other applications cannot even read the camera. `viva-camctl` takes `--access control|exclusive|monitor` on every command that opens a camera, and Python `connect_gige(..., exclusive=True)`. On USB3 Vision, where one application opens a camera at a time, exclusive is what every open already is, and monitor is refused. An Allied Vision Manta G-125B granted it (CCP read `0x1`) and answered another application's READREG and READMEM with `ACCESS_DENIED` ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **Watching another application's stream from the command line.** `viva-camctl stream --monitor` and `bench --monitor` open the camera read-only and receive the multicast stream another application set up, starting and stopping nothing. On the Manta G-125B the monitor received 123 frames while the controlling `stream --mode multicast` received all of its 308 complete ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **Python discovery by address.** `vg.discover_ip(ip)` asks one camera directly, for one on a routed subnet that broadcasts do not reach, and `vg.discover(allow_broadcast_ack=True)` lets a camera configured for another subnet broadcast its answer, as `viva-camctl list --ip` and `--allow-broadcast-ack` do ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **A GigE Vision hardware smoke suite.** `crates/viva-genicam/tests/gige_hardware.rs`, `#[ignore]`d and selected by `VIVA_GIGE_IP`, runs the parts of the hardware test plan a test can judge against a real camera; see [CONTRIBUTING → Tests](CONTRIBUTING.md#tests). All 19 tests pass on the Manta G-125B ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).

### Changed

- **A predicate that cannot be read is not satisfied.** A `pIsImplemented`, `pIsAvailable` or `pIsLocked` whose register the camera refuses made `NodeMap::feature_state` treat the feature as implemented, available and unlocked; it now reports it not implemented, not available, or locked, as the GenICam Standard defines a predicate by the value it reads. The Manta G-125B gates its `Lens*` features on an inquiry register at `0x23000` that it answers with `INVALID_ADDRESS`; they are now not implemented, and the warning is logged once per feature instead of three times. `viva-camctl dump` names on stderr each node it could not read instead of leaving it out silently ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **A takeover is seen on a camera that does not name its controller.** After a write refused with `ACCESS_DENIED`, a camera without GVCP Capability's CCP-application bit is asked by claiming control again (`GigeDevice::holds_control`): the Manta G-125B, which reads CCP as `0x2` to every application, accepted the claim from the holder and refused it to another. A session stopped past the heartbeat timeout while another took control now reports `ControlState::Lost` at its next write; before, it stayed `Held` ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).

### Fixed

- `viva-camctl list --allow-broadcast-ack` and `viva-camctl report` find a camera configured for another subnet on macOS: its broadcast acknowledgement reached the host but not the discovery socket, which was bound to the interface's own address. With broadcast acknowledgements allowed, discovery now binds the unspecified address and keeps the socket on its interface. Found on an Allied Vision Manta G-125B ([#215](https://github.com/VitalyVorobyev/viva-genicam/pull/215)).
- **`viva-camctl` logs go to stderr.** They went to stdout, so `viva-camctl dump --ip <CAM> > values.txt` wrote twelve `WARN` lines from the Manta G-125B into the file; stdout now carries only the data ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **Chunk mode on a GigE Vision 1.x camera.** With `ChunkModeActive` on, the Manta G-125B sends payload type `0x0005` (GigE Vision 1.x extended chunk data: an image leader, the image as the first chunk, and a trailer of payload length, Size Y and chunk layout ID), which the receive engine did not assemble, so the stream delivered nothing. Such a block is now an image with chunks, as `0x4001` is. `Camera::configure_chunks` with no selectors sets chunk mode alone on a camera with no `ChunkSelector`, as the Manta declares ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- **An event the camera resends is delivered once.** A camera resends an event until it is acknowledged, and the acknowledgement goes out when the application reads it: the Manta G-125B (`GevMCTT` 100 ms) sent one `ExposureEnd` eleven times, request id 1 each, to a reader a second late, and each copy came back as an event. A copy with the same request id and the same bytes is now acknowledged and dropped; distinct events under one request id are each delivered ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).

### Run on hardware

- On the Allied Vision Manta G-125B (firmware 00.01.44.28514, GigE Vision 1.2): a command register clears by the first `IsDone` read (`UserSetLoad`, done 15 ms after the write); `WriteAround` registers (`BlackLevel`, `Gamma`, `StreamBytesPerSecond`) read back the value just written; a second application's claim and its writes are refused with `ACCESS_DENIED` while one holds control, and writes from an application without control are refused while nobody holds it; a saved configuration loads back to the same 98 values; every pixel format it offers (`Mono8`, `Mono12`, `Mono12Packed`) streams complete frames, and five acquisition restarts in one stream lose none ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).

## Releases

| Series | Releases | File |
|--------|----------|------|
| 0.10 | 0.10.0 (2026-10-17) | [changelog/0.10.md](changelog/0.10.md) |
| 0.9 | 0.9.0 (2026-10-13) | [changelog/0.9.md](changelog/0.9.md) |
| 0.8 | 0.8.0 (2026-10-10) | [changelog/0.8.md](changelog/0.8.md) |
| 0.7 | 0.7.0 (2026-09-30) | [changelog/0.7.md](changelog/0.7.md) |
| 0.6 | 0.6.0 (2026-09-24) | [changelog/0.6.md](changelog/0.6.md) |
| 0.5 | 0.5.0 (2026-08-26) | [changelog/0.5.md](changelog/0.5.md) |
| 0.4 | 0.4.1 (2026-08-03), 0.4.0 (2026-07-31) | [changelog/0.4.md](changelog/0.4.md) |
| 0.3 | 0.3.1 (2026-07-31), 0.3.0 (2026-07-30) | [changelog/0.3.md](changelog/0.3.md) |
| 0.2 | 0.2.8 (2026-07-28), 0.2.7 (2026-07-28), 0.2.6 (2026-07-25), 0.2.5 (2026-04-15), 0.2.4 (2026-04-13), 0.2.3 (2026-04-12), 0.2.2 (2026-04-12), 0.2.1 (2026-04-12), 0.2.0 (2026-04-11) | [changelog/0.2.md](changelog/0.2.md) |
| 0.1 | 0.1.0 (2026-04-10) | [changelog/0.1.md](changelog/0.1.md) |

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.10.0...HEAD
