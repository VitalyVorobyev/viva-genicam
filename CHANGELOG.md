# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

### Read before upgrading

- **Chunk values come from the camera's XML.** `ChunkKind`, `ChunkValue`,
  `ChunkMap`, `parse_chunk_bytes` and `Frame::chunk` are removed: `Frame::chunks`
  holds the frame's chunks by ID, and `Camera::with_chunks` /
  `Camera::chunk_features` read them as features. For an image with chunks,
  `Frame::payload` is now the image alone. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- **`viva_gige::gvsp::parse_chunks` reads the GigE Vision layout**, from the end
  of the payload, and returns a `Result`; `ChunkRaw` borrows the payload and its
  `id` is a `u32`. `GvspTrailer` reports the chunk payload length and layout ID
  in place of a chunk region. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- **A register on a chunk or event port no longer reads device memory**: with
  no frame or event attached it fails with `GenApiError::NotAttached`, and
  writing it fails with `PortNotWritable`. `NodeMap::register_address` refuses
  it. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- **XML model** (consumed by Viva Studio): `<pPort>` is part of `Addressing`
  (`RegisterDecl::port` moved there), `AddressTerm::Fixed` is signed, and
  `XmlModel::ports` lists the `<Port>` declarations. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- **`Event` gains `stream_channel`, `block_id` and `raw`**, the event as the
  camera sent it; `viva_gige::message::EventPacket` gains `raw`. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
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
  `frames_lost()`, and `resends`/`resend_ranges` give way to the resend
  counters below. Only the stream records them. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`Stream::socket()` and `FrameStream::socket()` are removed**, with the
  legacy `gvsp::parse_packet`/`GvspPacket` and the unused reassembly types
  (`Reassembler`, `FrameQueue`, `BufferPool`, `ResendPlanner`,
  `GigeDevice::negotiate_stream`). Read frames through `FrameStream`.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **`viva-camctl bench --json-out` reports the engine's counters**:
  `frames`/`drops`/`resends` become `frames_complete`, `frames_incomplete`,
  `frames_missed`, `frames_backpressure`, `packets_received` and
  `packets_missing`; `stream` and `bench` print the same counters. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **A stream binds a port of its own.** The library, Python and Viva Studio
  received on port 24575 (`0x5FFF`) and `viva-camctl` on 10040; now the
  operating system picks a free port for each stream and the camera is told
  it. Behind a firewall that admits only a known port, name one with
  `destination_port` or `--port`. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- **`GevSCPD` is written only when asked**, and in time:
  `StreamOptions::packet_delay(Duration)` and `StreamBuilder::packet_delay`
  (which took raw ticks) convert it to the camera's ticks. Every stream used to
  write it — 25 ticks for packets of 1 500 bytes or less, 0 above — over the
  camera's own setting. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- **`GigeDevice::request_resend` is removed**, with `StreamConfig::resend_enabled`;
  its 8-byte request matched neither Wireshark's dissector nor aravis, and
  nothing called it. Resend is now the stream's (below).
  `StreamParams` gains `channel` and `camera`, and its `packet_delay` is an
  `Option`. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- **`viva-zenoh-api` payloads are `#[non_exhaustive]`** with constructors
  (`DeviceStatus::connected`, `AcquisitionStatus::active`, `ImageMeta::new`, …),
  so a later field is not a breaking change. `AcquisitionStatus.dropped` now
  counts every frame not delivered complete. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))

### Added

- Chunk and event features read through the camera's own XML (ADR-0026):
  `Camera::with_chunks` and `chunk_features` for a frame, `with_event` and
  `event_features` for an event; underneath, `NodeMap::attach_chunks`,
  `attach_event`, `ports` and `port_features`. Every register on a chunk or
  event port reads its chunk or event at the address its XML gives, negative
  addresses counting from the end of a chunk. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- The receive engine assembles chunk-data blocks (payload type `0x0004`). ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- `Camera::open_stream(StreamOptions)`: one call that claims control and only
  then configures the stream channel (multicast included), negotiates the
  packet size and starts the receive engine with the camera's `PayloadSize`.
  Studio, `viva-service`, `viva-camctl` and Python now open streams through it.
  ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194), [#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- `FrameStream::next_block_blocking` and `next_frame_blocking` for code without
  an async runtime, `set_deliver_incomplete` to receive frames that lost
  packets, with a status saying how, and `from_socket` for a camera configured
  elsewhere. ([#194](https://github.com/VitalyVorobyev/viva-genicam/pull/194))
- **Packet resend.** A stream asks a camera whose capability register says it
  supports it to send lost packets again, in the layout Wireshark's dissector
  reads, from the stream's own socket; a frame that got its packets back is
  complete. `resend_requested`, `resend_recovered` and `resend_unavailable`
  count it; `ResendPolicy::Off`, `--no-resend` or `resend=False` turn it off.
  Tested against the fake camera only. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- `--packet-delay-ns` on `viva-camctl stream` and `bench`, `packet_delay_ns=`
  and `resend=` on Python's `stream()`; `bench --json-out` adds the resend
  counters. The report bundle records GVCP Capability and the timestamp tick
  frequency. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- `Camera::execute_and_wait`, `Camera::is_done` and `NodeMap::is_done`
  (GenICam `IsDone`), so a caller can wait for a command such as `UserSetSave`
  to finish instead of sleeping. `viva-camctl execute` and Python's
  `Camera.execute` wait by default (`--no-wait`/`--wait-ms`;
  `wait=`/`timeout=`); a camera that has not reported completion in time is a
  warning there, not an error, since the command was sent. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- Packed and Y'CbCr pixel formats: `viva_pfnc::PixelFormat` names Mono10p,
  Mono12p, Mono10Packed, Mono12Packed, Bayer at 10 and 12 bits (unpacked, `p`
  and `Packed`), RGBa8/BGRa8 and 8-bit YUV/Y'CbCr 4:1:1, 4:2:2 and 4:4:4, with
  codes from EMVA's values table. `bits_per_pixel`, `line_bytes` and
  `image_bytes` size them exactly. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
- `viva_pfnc::convert`: unpack to `u16`, reduce to 8 bits, bilinear Bayer
  demosaic and Y'CbCr to RGB. `Frame::to_rgb8` and Viva Studio's viewer both
  use it, so Studio shows the pixels the library returns, and both now handle
  all of the formats above. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
- Frames saved at the camera's bit depth: the `image-io` feature's
  `viva_genicam::image_io` writes 16-bit PNG (depth in `sBIT`) and TIFF, and
  `viva-camctl stream --save-format png|tiff` uses it; PGM/PPM stays the
  default. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))

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
- `Frame::to_rgb8` demosaics bilinearly (it copied from the 2×2 cell) and
  ignores bytes past the image instead of rejecting the frame. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
- The services publish the camera's pixel format by name for every format
  Studio can draw: Mono10/12/14, Bayer above 8 bits and the packed formats were
  published as `Unknown`; the wire `PixelFormat` gains 28 variants. `ImageMeta`
  reads legacy names such as `YUV422Packed`, and its `payload_size` is the
  camera's `PayloadSize`, or the exact packed size without one. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
- Viva Studio's Linux bundles (AppImage and deb) are built on Ubuntu 24.04, so
  they need glibc 2.39 or newer — Ubuntu 24.04, Debian 13, Fedora 40 or later.
  The Ubuntu 22.04 build image retires in April 2027.
  ([#192](https://github.com/VitalyVorobyev/viva-genicam/pull/192))

### Fixed

- 518 registers on chunk and event ports in 19 vendor documents read device
  memory at their port-relative address: `EventExposureEndTimestamp` returned
  GVCP bootstrap bytes, and 2-byte event registers at 0x2 went out as unaligned
  `READMEM`s — the likely cause of the `BAD_ALIGNMENT` errors on
  [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), not yet
  confirmed on that camera. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
- Chunk data was looked for in the GVSP trailer, front to back, with 16-bit
  IDs, and decoded from a four-entry table; it is read from the payload, from
  the end, with the 32-bit IDs cameras declare. Baumer HXG20's
  `ChunkImageLength`, at a negative address, now builds. ([#197](https://github.com/VitalyVorobyev/viva-genicam/pull/197))
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
- Two streams on one host shared a port and split each other's packets; any
  sender could feed a stream. A stream now accepts only its camera's
  address, counted in `packets_ignored_foreign` otherwise. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- A stream channel the camera does not have (`GevNumberOfStreamChannels`) was
  configured anyway; it is refused before anything is written. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- A camera sending smaller data packets than the host expected could have its
  frame delivered as complete, with gaps and shifted data; it is now
  incomplete. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- On macOS the interface MTU always read as 1 500, so automatic packet sizing
  never chose jumbo frames. ([#195](https://github.com/VitalyVorobyev/viva-genicam/pull/195))
- `PixelFormat::BayerGR16`, `BayerRG16`, `BayerGB16` and `BayerBG16` carried the
  codes of BayerGB10, BayerBG10, BayerGR12 and BayerRG12, so a camera sending
  one of those was read as 16-bit Bayer of another pattern. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
- A packed format is sized exactly when the camera has no `PayloadSize`:
  `U3vStreamBuilder` refused it, and `viva-camctl stream-usb` sized it at one
  byte a pixel, a third short for `Mono12Packed`. ([#196](https://github.com/VitalyVorobyev/viva-genicam/pull/196))
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
