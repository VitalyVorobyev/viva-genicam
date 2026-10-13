# ADR-0032: The USB3 Vision transport: nusb, transfers in flight, recovery and identity

**Status:** Accepted; supersedes [ADR-0015](adr0015-vendored-libusb-lgpl-notices.md)
**Date:** 2026-10-13

## Context

Up to 0.8 USB3 Vision streamed with one synchronous `rusb` bulk read at a
time, copied each payload once more, and ended the stream at the first USB
error. Nothing reset the stream endpoint before enabling it, an unplug looked
like any other error, devices were keyed by bus and address, and discovery
opened every device to read its strings. The facade's `U3vFrameStream` read on
a Tokio runtime's blocking pool, so the blocking API
([ADR-0027](adr0027-blocking-api-and-transport-runtimes.md)) could not stream
from a USB camera, and USB3 Vision had no chunks, events, stream counters or
time sync. The Python wheel linked libusb statically, which made the package
`MIT AND LGPL-2.1-or-later` (ADR-0015). Backlog rows UV-01 to UV-04, API-11
and TC-27 (3) and (4) recorded these gaps.

No USB3 Vision camera was on record as having run any of it (TC-27; the
tracker holds no USB3 Vision report). During this work one did: a MATRIX
VISION mvBlueFOX3-2004G (`164c:5533`), on macOS arm64, over a USB 2 High
Speed link behind a hub. The [evidence hierarchy](../../CLAUDE.md#evidence-hierarchy)
puts it above everything else this ADR cites, and where it disagreed with the
fake, the fake changed (§8).

## Decision

### 1. nusb, not rusb

USB goes through [nusb](https://crates.io/crates/nusb) 0.2:

- **It queues transfers.** An endpoint takes several transfers at once
  (`submit`, `wait_next_complete`, `cancel_all`). rusb 0.9 has no asynchronous
  interface; `rusb-async` is a 0.0.1 alpha.
- **Discovery opens nothing.** Strings and link speed come from the operating
  system's device list, so a camera another process holds is listed with its
  serial, and discovery cannot disturb a camera the service is streaming from.
  This settled UV-04 on macOS: a service holding the camera rescanned six times
  and kept serving it under the same id.
- **It is pure Rust.** No C library is built or linked anywhere, so the Python
  package is MIT again and ADR-0015's notices go. Its MSRV (1.85) is below ours.

What it costs: a younger crate, and two rules of its own. An IN transfer's
length must be a multiple of the endpoint's max packet size, so a stream's
transfer sizes are aligned to the larger of the SIRM alignment and that size
(512 on the High Speed camera). On macOS a transfer the host aborts completes
as `Cancelled`, which §3 has to tell apart from a cancel of our own.

### 2. Several transfers in flight

`UsbTransfer::bulk_in(endpoint)` hands out a `BulkIn` queue. nusb's queues
transfers. The trait's default runs them one at a time, which keeps simple
transports working. `U3vStream` keeps `DEFAULT_TRANSFERS_IN_FLIGHT` (8)
queued, each at most `DEFAULT_MAX_TRANSFER_SIZE` (1 MiB). It plans each
transfer from where the block stands: the leader, the payload at some offset,
or the trailer. It then checks every completion against that plan. When the
two part — a block that ended early, a trailer for another block — it cancels
what is queued, reads what had already arrived, and plans again from what it
read. A payload is copied once, from its transfers into the block's buffer.
When the whole payload arrives in one transfer it is not copied at all.

### 3. A failed transfer costs its block, not the stream

A stall, any other transfer error, or a transfer the host aborted gives up the
block in progress. The stream then drains the queue, resets the endpoint and
waits for the next leader. The reset is `SET_FEATURE(ENDPOINT_HALT)` followed
by `CLEAR_FEATURE`, as aravis's `arv_uv_device_reset_stream_endpoint` does.
The stream ends only after eight failures in a row. `Disconnected` ends it at
once: the facade marks the camera lost (`U3vRegisterIo::is_lost`), a later
register access does the same, and `viva-service` retires a lost camera
instead of polling it. Which errors a camera actually recovers from is an open
question (UV-05).

**A disconnect is decided by the device list, not by the error alone.** When
the mvBlueFOX3-2004G's cable was pulled mid-stream on macOS, no transfer said
the device was gone. The bulk IN transfer in flight failed with
`kIOReturnNotResponding` (`0xe00002ed`). The next control transfers failed
with `kIOReturnNotReady` (`0xe00002d8`) and `kIOReturnNotAttached`
(`0xe00002d9`). `nusb` 0.2.7, the latest release, maps only
`kIOReturnNoDevice` to `TransferError::Disconnected`, so all three arrived as
unknown errors, and the stream ended with one of them. So a transfer that
fails without saying why (`U3vError::Usb`, or a cancel we did not ask for)
asks whether the operating system still lists the device
(`UsbTransfer::is_present`; `NusbTransfer` matches `nusb`'s device id in
`list_devices`). If the device is not listed, the error is `Disconnected`, on
the stream, event and control paths alike. A timeout, a stall or anything the
device answered is never checked. The stream looks once before it recovers.
An unplugged device can still be listed for a moment, so a failure after
that — the endpoint reset, a register access — waits up to 500 ms for the
device to leave the list. The IOKit codes are named in the error's text but
decide nothing on their own: `kIOReturnNotResponding` can be transient.

### 4. Reset an endpoint before enabling it

The stream endpoint is reset after the SIRM is configured and before
`StreamEnable`. The event endpoint is reset before `EventEnable`. On the
camera, the stream reset makes block ids restart at 0 with every
acquisition; without it they continued from the last run. The event reset
discards events left over from an earlier acquisition. Before it was added,
those stale events arrived first, and since block ids restart at 0, they
matched the wrong frames.

### 5. A camera is keyed by what it is

`viva_zenoh_api::u3v_device_id` is `cam-u3v-<vendor><product>-<serial>`, so a
camera keeps its id on another port. A camera without a serial falls back to
`<bus>-<address>`. Characters a key expression cannot carry become `_`. The
device GUID is not used: reading it needs device I/O, and discovery does none
(§1). On the camera, the ABRM serial, the USB serial string and
`DeviceSerialNumber` were the same text.

### 6. The facade: GigE's shapes, on a thread of its own

`U3vFrameStream` reads on a `std::thread` of its own and delivers frames
through GigE's drop-oldest `DeliveryQueue`, counted in GigE's `StreamStats`
(plus `transfer_errors`). Both the async `next_frame` and
`next_frame_blocking` work without a runtime. A stream is opened by
`Camera::open_stream(U3vStreamOptions)` or by
`U3vStreamBuilder::new(&U3vRegisterIo)`. The builder's `build()` stays
synchronous, while GigE's is async. Opening a USB stream is register writes
over a synchronous `RegisterIo` and starting a thread, so there is nothing to
await. GigE's builder awaits socket setup and a packet-size probe.

Chunks use GigE Vision's layout with little-endian tags
(`viva_u3v::parse_chunks`); `Camera::with_chunks` reads them as ADR-0026 does.
Events are GenCP `EVENT_CMD`s on the event endpoint, enabled through the EIRM
(`Camera::open_event_stream`). The EIRM offsets come from Wireshark's
`packet-u3v.c`. `time_calibrate` reads GenCP's timestamp registers through
`Timestamp`/`TimestampIncrement` as well as SFNC's names. The trailer's
`Size Y` becomes `Frame::delivered_height`. A trailer reporting an error
becomes `FrameStatus::U3vDeviceStatus`.

### 7. A camera below SuperSpeed is warned about when it is opened

`U3vDevice::open` logs a warning when the link, or failing that the SBRM's
`CurrentSpeed`, is below SuperSpeed. Discovery does not warn, because the
service rescans every few seconds.

### 8. What the camera confirmed, and what it changed

| Question | The mvBlueFOX3-2004G's answer |
|---|---|
| Bootstrap | GenCP 1.0, SBRM `0x10000`, SIRM `0x20000`, EIRM `0x40000`; SIRM alignment 16; `CurrentSpeed` = High Speed |
| Transfer arithmetic | Leader, payload, final transfer and trailer, each aligned to 512: four transfers per 396 032-byte frame, every block complete |
| Transfers in flight | Full ROI at the camera's maximum rate: 106.7 fps (42.2 MB/s) for 0.8's one read at a time, 113.5 fps (45.0 MB/s) with 8 in flight. 2: 42.9 MB/s; 32: unchanged. Transfer size (16 KiB to 1 MiB) changed nothing. 45 MB/s is 75% of the camera's `DeviceLinkSpeed` (60 MB/s): the USB 2 link is the limit |
| Chunks | Little-endian tags, as aravis reads them. Every value sits in one chunk (ID `0x44455569`), and the image chunk's ID is `0x11223344`. `ChunkTimestamp` equals the leader's timestamp, and `ChunkExposureTime`, `ChunkWidth`, `ChunkHeight` and `ChunkPixelFormat` equal their features |
| Events | `EVENT_CMD` with ids `0x9000` (ExposureEnd) and `0x9001` (FrameEnd). The stream channel is a u32 at `0xC` and the frame id a u64 at `0x10`; frame ids equal block ids. ExposureEnd's timestamp is 20.11 ms after the frame's at ExposureTime 20 ms, so the leader's timestamp is the start of exposure |
| Timestamps | The GenCP registers are named `Timestamp`, `TimestampLatch` and `TimestampIncrement`. `time_calibrate` could not find them under SFNC's names alone |
| Unplug (macOS) | `kIOReturnNotResponding` on the stream, `kIOReturnNotReady` and `kIOReturnNotAttached` on control; never `kIOReturnNoDevice`. The device was gone from `list-usb` afterwards. Hence §3's device-list check. A second unplug, with the check: `kIOReturnNotResponding` on the stream, the device gone from the list 35 ms later, `Disconnected`, and `AcquisitionStop` reporting the disconnect |

The fake changed to match. It can be unplugged as macOS reported it
(`unplug_as_macos_does`): unknown IOKit codes, and the device still listed
for a moment. It restarts block ids at 0 after an endpoint reset
and starts at 0 rather than 1. Its event data and timestamp feature names
follow the camera's. Its image chunk id is `0x11223344`, and it paces blocks
at `AcquisitionFrameRate` rather than as fast as they are read.

## Consequences

### Positive

- Streaming survives a failed transfer. Running at the link's limit it moves
  7% more data, and it counts the blocks the camera dropped, which 0.8 never
  saw.
- An unplugged camera ends its stream with an error of its own. The service
  stops serving it.
- USB3 Vision has chunks, events, counters, time sync and the blocking API,
  in the same types as GigE Vision.
- No C dependency: the Python package is MIT, and no wheel ships
  third-party notices.
- A camera keeps its id across ports and rescans.

### Negative

- All of the camera evidence is one camera, one OS and a USB 2 link. Linux,
  Windows, SuperSpeed, other vendors and a stall in the middle of a stream are
  untried, and an unplug has been seen only on macOS (TC-27).
- A transfer error on a device still listed costs one look at the device
  list, and a failure that follows it up to 500 ms more.
- Which transfer errors are transient is unknown. Eight failures in a row is
  a guess (UV-05).
- A camera that asks for an event acknowledgement cannot get one: the event
  interface has no OUT endpoint. We log it once (UV-06).
- `U3vDeviceInfo` changed shape (`bus` is text; it gained `speed`; it lost
  `interface_info`), and so did the device id. Both are breaking changes.
