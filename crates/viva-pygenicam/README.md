# viva-genicam (Python)

Pure-Rust GenICam stack with Python bindings. Discover, control, and stream GigE Vision and USB3 Vision cameras from Python — no aravis, no C toolchain, just a wheel.

```bash
pip install viva-genicam
```

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
cam = vg.connect_gige(cams[0])

print(cam.get("DeviceModelName"))
cam.set_exposure_time_us(10_000.0)

with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()          # NumPy (H, W) or (H, W, 3) uint8
        print(frame.width, frame.height, frame.pixel_format)
        break
```

See the [documentation](https://vitalyvorobyev.github.io/viva-genicam/python.html) for the full API.

## Build from source

```bash
uv venv .venv
uv pip install --python .venv/bin/python maturin numpy pytest
uv run --python .venv/bin/python maturin develop -m crates/viva-pygenicam/Cargo.toml
```
