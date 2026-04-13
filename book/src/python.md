# Python bindings

The `viva-genicam` Python package wraps the Rust workspace behind a NumPy-friendly API. It ships as a pre-built wheel on PyPI — no C toolchain, no aravis, just `pip install`.

```bash
pip install viva-genicam
```

## Quickstart

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
cam = vg.connect_gige(cams[0])

print(cam.get("DeviceModelName"))
cam.set_exposure_time_us(10_000.0)

with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()            # (H, W) or (H, W, 3) uint8
        print(frame.width, frame.height, frame.pixel_format)
        break
```

## Discovery

```python
vg.discover(timeout_ms=500, iface="en0")   # restrict to one NIC
vg.discover(timeout_ms=500, all=True)       # enumerate every interface
vg.discover_u3v()                            # USB3 Vision cameras
```

Returns a list of `GigeDeviceInfo` / `U3vDeviceInfo` frozen dataclasses with `.ip`, `.mac`, `.manufacturer`, `.model`.

## Camera

`Camera` is one class for both GigE and U3V. Methods:

- `get(name) -> str`, `set(name, value)` — string-based feature access
- `set_exposure_time_us(float)`, `set_gain_db(float)` — typed conveniences
- `nodes() -> list[str]`, `node_info(name) -> NodeInfo`, `all_node_info()`, `categories()`
- `enum_entries(name) -> list[str]`
- `acquisition_start()`, `acquisition_stop()`
- `stream(iface=..., auto_packet_size=...) -> FrameStream`

`NodeInfo` exposes `name`, `kind` ("Integer", "Float", …), `access` ("RO"/"RW"/"WO"), `visibility`, `description`, `tooltip`, plus `.readable` / `.writable` properties.

## Streaming

`camera.stream()` returns a context manager that starts acquisition on entry, stops on exit, and yields `Frame` objects:

```python
with cam.stream() as frames:
    for frame in frames:
        rgb = frame.to_rgb8()           # always (H, W, 3) uint8
        # or
        arr = frame.to_numpy()           # natural shape per pixel format
        ...
```

Frames carry `.width`, `.height`, `.pixel_format`, `.pixel_format_code`, `.ts_dev`, `.ts_host`, and `.payload()` for raw bytes.

## Errors

All exceptions subclass `vg.GenicamError`:

```
GenicamError
├── GenApiError               (nodemap evaluation)
├── TransportError            (register I/O / discovery / streaming)
├── ParseError                (invalid user input)
├── MissingChunkFeatureError  (chunk selector absent from XML)
└── UnsupportedPixelFormatError
```

## Build from source

```bash
uv venv .venv
uv pip install --python .venv/bin/python maturin numpy pytest
uv run --python .venv/bin/python maturin develop -m crates/viva-pygenicam/Cargo.toml --release
```

Linux/macOS need `libusb` installed (`apt install libusb-1.0-0-dev` / `brew install libusb`) for USB3 Vision support.
