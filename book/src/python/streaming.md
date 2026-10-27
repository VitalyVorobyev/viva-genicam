# Streaming

`Camera.stream()` returns a context manager that starts acquisition on entry, stops it on exit, and yields `Frame` objects while it's open:

```python
with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()
        ...
```

No asyncio required — the stream's receive engine runs on a thread of its own inside the extension, and iteration blocks the calling thread while other Python threads stay runnable (the GIL is released during the blocking read). The stream, and every frame, may be handed to another thread, for a producer–consumer loop.

Only complete frames come out of the iterator, unless you ask for the others
(see [Incomplete frames](#incomplete-frames)). A frame that lost packets on the
network is counted by the stream and skipped. Gaps can also come from host
backpressure: the bounded delivery queue drops
old pending frames if the application falls behind. Check the stream counters
to distinguish those cases.

## The `Frame` object

```python
frame.width              # int
frame.height             # int
frame.pixel_format       # "Mono8", "Mono12p", "BayerRG8", "RGB8Packed", ...
frame.pixel_format_code  # raw PFNC integer
frame.block_id           # the camera's number for the frame
frame.status             # vg.FrameStatus.COMPLETE, unless incomplete frames were asked for
frame.ts_dev             # device tick count, Optional[int]
frame.ts_host            # POSIX seconds, Optional[float]: None until cam.time_calibrate()
frame.chunks             # {chunk_id: bytes}, or None without chunk mode
frame.payload()          # raw bytes, one copy
```

`ts_host` is the frame's device timestamp mapped onto the host clock, which
needs a fit of the two clocks first. On a GigE camera, call
`cam.time_calibrate()` before opening the stream; without it `ts_host` is
`None`. `cam.map_dev_ts(ticks)` maps any device timestamp the same way.

### `to_numpy()` — the camera's values

For high-rate acquisition, import NumPy (`import numpy`) before opening the
stream. Otherwise its first import happens during conversion and can pause the
consumer long enough to overflow the delivery queue. Keep file I/O and heavy
processing out of the acquisition loop, and check host-drop counters as well as
network loss.

```python
arr = frame.to_numpy()
```

| Pixel format | Array shape | dtype |
|---|---|---|
| Mono8 | `(H, W)` | `uint8` |
| Mono10, Mono12, Mono14, Mono16, and the packed Mono10p, Mono12p, Mono10Packed, Mono12Packed | `(H, W)` | `uint16` |
| Bayer, any depth | `(H, W)` — the mosaic as the camera sent it | `uint8` for 8 bits, else `uint16` |
| Bayer with `to_numpy(demosaic=True)` | `(H, W, 3)` | the same depth |
| RGB8, BGR8 (reordered to RGB), with or without alpha, and 8-bit Y'CbCr | `(H, W, 3)` | `uint8` |
| anything else | `(N,)` raw bytes | `uint8` |

A 16-bit array holds the camera's values, not values scaled to 16 bits: a
12-bit pixel is `0` to `4095`. Line padding the camera declares is skipped.
Demosaicing is bilinear, the kernel Viva Studio's viewer uses — fine for
preview, not a substitute for an ISP.

<!-- pytest: none -->
```python
# Mono12p: two 12-bit pixels, 0xABC and 0x123, packed least significant bit first.
frame = vg.Frame.from_bytes(bytes([0xBC, 0x3A, 0x12]), 2, 1, "Mono12p")
arr = frame.to_numpy()
assert arr.dtype == np.uint16 and arr.tolist() == [[0xABC, 0x123]]
```

`Frame.from_bytes(payload, width, height, pixel_format, padding_x=0)` builds a
frame from bytes saved earlier or received elsewhere, for the same conversions.

### `to_rgb8()` — always RGB

```python
rgb = frame.to_rgb8()     # always (H, W, 3) uint8
```

Useful when you want one code path regardless of the camera's pixel format. It
converts monochrome and Bayer at 8 to 16 bits, packed formats such as
`Mono12p` and `Mono12Packed` included, RGB and BGR with or without alpha, and
8-bit YUV; samples deeper than 8 bits keep their top eight. Other formats raise
`UnsupportedPixelFormatError`.

### Raw bytes

```python
frame.payload()           # bytes, copy of the whole GVSP payload
```

Use this when you need to feed bytes to another decoder or serialize to disk as-is.

## Streaming options

The `stream()` call accepts GigE-specific knobs:

```python
cam.stream(
    iface="en0",               # NIC override — or "192.168.0.5"
    auto_packet_size=True,     # size packets from the NIC MTU (or packet_size=N)
    multicast="239.255.42.99", # have the camera stream to a multicast group, and join it
    destination_port=34567,    # a fixed UDP port, for a firewall; default: the OS picks one
    packet_delay_ns=2000,      # GevSCPD, converted to the camera's ticks; default: left alone
    resend=False,              # never ask for lost packets; default: ask if the camera can
)
```

None of these are required. `iface=` is auto-resolved by subnet match if you omit it; the rest fall back to the defaults noted above. What packet delay and resend do is in [Streaming → Packet delay](../tutorials/streaming.md#42-packet-delay) and [Packet resend](../tutorials/streaming.md#46-packet-resend).

Packet size has three settings. Omit both arguments and the camera's current
`GevSCPSPacketSize` is kept. `auto_packet_size=True` sets it from the host
NIC's MTU; `packet_size=N` writes `N` as a ceiling. Pass at most one of the
two. In every case a test-packet probe may lower the size if the network path
drops it — see [Streaming → Packet size and MTU](../tutorials/streaming.md#41-packet-size-and-mtu).

`iface=` takes the host NIC's IPv4 address or its OS name, the same two spellings `viva-camctl --iface` and `viva-service --iface` accept — so the address you read off `discover()` is a legal value here.

For U3V cameras all options are silently ignored.

### Incomplete frames

```python
with cam.stream(deliver_incomplete=True) as frames:
    for frame in frames:
        if frame.status is vg.FrameStatus.INCOMPLETE:
            print(frame.block_id, "lost", frame.missing_packets, "packets")
```

Lost packets' bytes are zero in the payload. Every incomplete frame is counted
in the stream's statistics either way.

### Counters

<!-- pytest: camera -->
```python
with cam.stream() as frames:
    frames.next_frame(timeout_ms=5000)
    stats = frames.stats()
print(stats.frames_complete, stats.frames_lost, stats.resend_recovered)
```

`stats()` returns every counter of the GigE receive engine: each frame's fate
(complete, incomplete, missed, dropped because the application fell behind),
each packet's, the packets asked for again and what became of them, and
rates. What each counts is in
[Streaming → What the statistics tell you](../tutorials/streaming.md#43-what-the-statistics-tell-you).

### Watching another application's stream

A camera opened with `connect_gige(info, monitor=True)` claims and writes
nothing. It can watch a multicast stream the controlling application set up:

```python
with vg.connect_gige(info, monitor=True) as watcher:
    with watcher.stream(monitor=True) as frames:   # starts no acquisition
        frame = frames.next_frame(timeout_ms=5000)
```

The group, port and packet size come from the camera; only `iface=` may be
given. A USB3 Vision camera has no monitor access — one application opens the
device — so `stream(monitor=True)` on one raises `GenicamError`. See [Networking → One camera, several applications](../networking.md#61-one-camera-several-applications).

## Chunks

A camera in chunk mode sends values with each frame — its timestamp, its
exposure, its number. `configure_chunks` turns them on, and `chunk_features`
reads them through the camera's XML, like any other feature:

<!-- pytest: camera -->
```python
cam.configure_chunks(["FrameID", "Timestamp"])
with cam.stream() as frames:
    frame = frames.next_frame(timeout_ms=5000)
cam.configure_chunks([], active=False)
print(cam.chunk_features(frame))   # {"ChunkFrameID": "1", "ChunkTimestamp": "...", ...}
```

A value that could not be read maps to the exception that kept it, instead of
a string.

## Events

A camera reports what happens — an exposure ending, a frame starting — on its
message channel (GigE Vision) or its event interface (USB3 Vision):

<!-- pytest: camera -->
```python
with cam.events(["ExposureEnd"]) as events:
    with cam.stream():
        event = events.next_event(timeout_ms=5000)
print(hex(event.id), event.block_id, cam.event_features(event))
```

`event_features` reads the features the camera's XML derives from the event,
such as `EventExposureEndTimestamp`. On a GigE camera `events()` takes
`iface=` and `port=` as `stream()` does; a USB3 Vision camera needs neither.

## Timeouts and ending a stream

Iteration blocks until a frame arrives. To time out a single read:

```python
with cam.stream() as frames:
    frame = frames.next_frame(timeout_ms=1000)
    if frame is None:
        print("stream ended cleanly")
    else:
        ...
```

`next_frame()` returns `None` when the stream closes cleanly, or raises `TransportError` on timeout / network failure. The `for frame in frames` path uses a 5-second default timeout that raises on expiry.

Exit the `with` block to stop acquisition and release the socket / USB endpoint. You can also call `frames.close()` explicitly if you stored the iterator outside a `with` statement.

## Complete example: save 5 frames as PNGs

```python
import viva_genicam as vg
from PIL import Image

cam = vg.connect_gige(vg.discover(timeout_ms=500)[0])

with cam.stream() as frames:
    for i, frame in enumerate(frames, 1):
        Image.fromarray(frame.to_numpy()).save(f"frame_{i:03d}.png")
        if i >= 5:
            break
```

Identical in spirit to the Rust `grab_gige` example, 12 lines of Python.

## Next

→ [API reference](api.md) — every public symbol, in one page.
