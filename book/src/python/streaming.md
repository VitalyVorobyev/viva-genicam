# Streaming

`Camera.stream()` returns a context manager that starts acquisition on entry, stops it on exit, and yields `Frame` objects while it's open:

```python
with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()
        ...
```

No asyncio required — the underlying `tokio` runtime is managed inside the extension, and iteration blocks the calling thread while other Python threads stay runnable (the GIL is released during the blocking read).

## The `Frame` object

```python
frame.width              # int
frame.height             # int
frame.pixel_format       # "Mono8", "Mono16", "BayerRG8", "RGB8Packed", ...
frame.pixel_format_code  # raw PFNC integer
frame.ts_dev             # device tick count, Optional[int]
frame.ts_host            # POSIX seconds, Optional[float] (only if time-synced)
frame.payload()          # raw bytes, one copy
```

### `to_numpy()` — natural shape

```python
arr = frame.to_numpy()
```

| Pixel format | Array shape | dtype |
|---|---|---|
| Mono8 | `(H, W)` | `uint8` |
| Mono16 | `(H, W)` | `uint16` |
| RGB8Packed | `(H, W, 3)` | `uint8` |
| BGR8Packed, BayerRG8, BayerGB8, BayerBG8, BayerGR8 | `(H, W, 3)` | `uint8` (auto-demosaiced / reordered) |
| anything else | `(N,)` raw | `uint8` |

Demosaicing is a simple nearest-neighbour kernel inside the Rust `to_rgb8()` path — fine for preview, not a substitute for an ISP.

### `to_rgb8()` — always RGB

```python
rgb = frame.to_rgb8()     # always (H, W, 3) uint8
```

Useful when you want one code path regardless of the camera's pixel format.

### Raw bytes

```python
frame.payload()           # bytes, copy of the whole GVSP payload
```

Use this when you need to feed bytes to another decoder or serialize to disk as-is.

## Streaming options

The `stream()` call accepts GigE-specific knobs:

```python
cam.stream(
    iface="en0",               # NIC override
    auto_packet_size=True,     # negotiate the largest packet that fits MTU
    multicast="239.255.42.99", # subscribe to a multicast group instead of unicast
    destination_port=34567,    # fix the streaming UDP port
)
```

None of these are required. `iface=` is auto-resolved by subnet match if you omit it; the rest fall back to the camera's defaults.

For U3V cameras all options are silently ignored.

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
