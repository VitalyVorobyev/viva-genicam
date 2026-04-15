# Python bindings

The `viva-genicam` Python package wraps the Rust workspace behind a NumPy-friendly API. It ships as a pre-built wheel on PyPI — no C toolchain, no aravis, libusb is statically bundled.

```bash
pip install viva-genicam
```

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
cam = vg.connect_gige(cams[0])
print(cam.get("DeviceModelName"))

with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()           # NumPy (H, W) or (H, W, 3) uint8
        break
```

## Tutorials

1. [Install & hello-camera](python/install.md) — install the wheel, run the self-contained fake-camera demo.
2. [Discovery](python/discovery.md) — enumerate GigE and U3V cameras, restrict to one NIC, auto-detect interfaces.
3. [Control & introspection](python/control.md) — read and write features, walk the NodeMap, discover which features apply.
4. [Streaming](python/streaming.md) — context-manager streams, NumPy frames, pixel formats, timestamps.

## Reference

- [API reference](python/api.md) — every public class, function, and exception in one place.
- [Example scripts](https://github.com/VitalyVorobyev/viva-genicam/tree/main/crates/viva-pygenicam/examples) — runnable Python files mirroring the most common Rust examples.

## Supported

- Python 3.9+, abi3 wheels (one wheel covers every minor version).
- GigE Vision: discovery, control, streaming, chunks, events, time sync.
- USB3 Vision: discovery, control, streaming.
- Platforms with pre-built wheels: Linux x86_64 (manylinux_2_28), macOS arm64, Windows x86_64.

Need another platform? The sdist on PyPI builds from source — you'll need a Rust toolchain (`rustup`) and a C compiler. libusb is always statically vendored; no system package needed.
