# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

### Read before upgrading

- **Commands write the value their XML declares.** `<pCommandValue>` was
  ignored, so every such command wrote `1` (19 of 38 corpus documents). On a
  Hikrobot MV-CS050 with `UserSet2` selected, `UserSetSave` overwrote user
  set 1; Point Grey `UserSetSave` expects a key such as `0x80000000`. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- **Writes are checked against the limits the camera declares**, including
  `<pMin>`/`<pMax>`/`<pInc>`, before being forwarded through `<pValue>` — most
  `Width`/`Height`/`Offset` features were never checked. A value outside them
  now fails with `Range` before it reaches the camera. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- **`Frame` is `#[non_exhaustive]`** and gains `block_id`, `payload_type` (all
  16 bits) and `status`; build one with `Frame::new`. A stream delivers only
  complete frames unless asked for the others. `EvsBlock` gains the same
  `block_id` and `status`. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`FrameStream::set_frame_timeout` is now `set_block_timeout`**, and the
  timeout counts from a block's last packet rather than its first. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`StreamStats` is rewritten and moved to `viva_genicam::StreamStats`.** It
  counts every frame once — complete, incomplete, missed or dropped to
  backpressure — plus what became of the packets; `drops` becomes
  `frames_lost()`, and the resend counters are gone until resend exists. Only
  the stream records them. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`Stream::socket()` and `FrameStream::socket()` are removed**, with the
  legacy `gvsp::parse_packet`/`GvspPacket` and the unused reassembly types
  (`Reassembler`, `FrameQueue`, `BufferPool`, `ResendPlanner`,
  `GigeDevice::negotiate_stream`). Read frames through `FrameStream`.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`viva-camctl bench --json-out` reports the engine's counters**:
  `frames`/`drops`/`resends` become `frames_complete`, `frames_incomplete`,
  `frames_missed`, `frames_backpressure`, `packets_received` and
  `packets_missing`; `stream` and `bench` print the same counters. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`viva-zenoh-api` payloads are `#[non_exhaustive]`** with constructors
  (`DeviceStatus::connected`, `AcquisitionStatus::active`, `ImageMeta::new`, …),
  so a later field is not a breaking change. `AcquisitionStatus.dropped` now
  counts every frame not delivered complete. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))

### Added

- `Camera::open_stream(StreamOptions)`: one call that claims control,
  configures the stream channel (multicast included), negotiates the packet
  size and starts the receive engine with the camera's `PayloadSize`. Studio,
  `viva-service`, `viva-camctl` and Python now open streams through it.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- `FrameStream::next_block_blocking` and `next_frame_blocking` for code without
  an async runtime, `set_deliver_incomplete` to receive frames that lost
  packets, with a status saying how, and `from_socket` for a camera configured
  elsewhere. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- `Camera::execute_and_wait`, `Camera::is_done` and `NodeMap::is_done`
  (GenICam `IsDone`), so a caller can wait for a command such as `UserSetSave`
  to finish instead of sleeping. `viva-camctl execute` and Python's
  `Camera.execute` wait by default (`--no-wait`/`--wait-ms`;
  `wait=`/`timeout=`); a camera that has not reported completion in time is a
  warning there, not an error, since the command was sent. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))

### Changed

- **One GVSP receive engine on every OS** (ADR-0025): a thread per stream reads
  the socket whether or not the application is asking for a frame, so time
  spent on a frame no longer becomes packets the kernel drops; frames reach the
  application through a four-frame queue that drops the oldest when it falls
  behind, counted. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- The stream sizes its socket receive buffer to twice the frame (at least
  4 MiB) and warns once, naming `net.core.rmem_max` or `kern.ipc.maxsockbuf`,
  when the system grants under half. Studio and `viva-camctl stream` no longer
  ask for a fixed 64 MiB. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- Python: a frame stream may be used from another thread, and `multicast=` now
  also points the camera at the group. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- The fake camera sends 16-bit and packed formats at their real size (Mono16
  was one byte a pixel), honours `GevSCPD` in timestamp ticks, and says `0x4001`
  in the leader as well as the trailer when chunks are on. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- A command checks its own `pIsImplemented`/`pIsAvailable`/`pIsLocked` before
  writing, on every addressing path. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- `<pSelected>` is read as the standard defines it, on the selector: writing a
  selector refreshes the features it selects. The non-standard per-node
  `<Selected>` addressing is removed. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- `feature_state` reports dynamic limits, a delegating feature's inherited
  ones, and a converter's range in the order its `<Slope>` gives. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- XML model (consumed by Viva Studio): `Integer`/`Float` bounds are `Option`;
  `Integer` gains `p_inc` and `selected`, `Float` gains `p_min`/`p_max`;
  `Command::command_value` is a `CommandValueSrc`; `Addressing::BySelector`,
  `selectors` and `selected_if` are gone. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- Viva Studio's Linux bundles (AppImage and deb) are built on Ubuntu 24.04, so
  they need glibc 2.39 or newer — Ubuntu 24.04, Debian 13, Fedora 40 or later.
  The Ubuntu 22.04 build image retires in April 2027.
  ([#192](https://github.com/VitalyVorobyev/viva-genicam/pull/192))

### Fixed

- A frame whose transfer took longer than 100 ms never completed: the timeout
  counted from the leader. Large frames and paced links now complete.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- Extended-ID streams (a 20-byte header, as LUCID cameras send) were assembled
  12 bytes off per packet on Linux and macOS. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- A trailer arriving before the last data packet, a repeated leader, or two
  blocks interleaved on the wire cost the frame. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- Packed formats such as `Mono12Packed` were truncated by a third on Windows.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- Payload type `0x4001` (image with chunks) kept only its low byte; other
  payload types were dropped without a count. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- Writes to a floating `<Integer>` (a `<Value>` with no register) always
  failed; 319 corpus features write through one, among them AVT's
  `TriggerSelector` and Sony's `UserSetSelector`. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))

## Releases

| Series | Releases | File |
|--------|----------|------|
| 0.8 | 0.8.0 (2026-10-10) | [changelog/0.8.md](changelog/0.8.md) |
| 0.7 | 0.7.0 (2026-09-30) | [changelog/0.7.md](changelog/0.7.md) |
| 0.6 | 0.6.0 (2026-09-24) | [changelog/0.6.md](changelog/0.6.md) |
| 0.5 | 0.5.0 (2026-08-26) | [changelog/0.5.md](changelog/0.5.md) |
| 0.4 | 0.4.1 (2026-08-03), 0.4.0 (2026-07-31) | [changelog/0.4.md](changelog/0.4.md) |
| 0.3 | 0.3.1 (2026-07-31), 0.3.0 (2026-07-30) | [changelog/0.3.md](changelog/0.3.md) |
| 0.2 | 0.2.8 (2026-07-28), 0.2.7 (2026-07-28), 0.2.6 (2026-07-25), 0.2.5 (2026-04-15), 0.2.4 (2026-04-13), 0.2.3 (2026-04-12), 0.2.2 (2026-04-12), 0.2.1 (2026-04-12), 0.2.0 (2026-04-11) | [changelog/0.2.md](changelog/0.2.md) |
| 0.1 | 0.1.0 (2026-04-10) | [changelog/0.1.md](changelog/0.1.md) |

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.8.0...HEAD
