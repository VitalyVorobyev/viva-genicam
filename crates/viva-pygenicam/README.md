# viva-genicam (Python)

Pure-Rust GenICam stack with Python bindings. Discover, control, and stream GigE Vision and USB3 Vision cameras from Python — no aravis, no C toolchain, just a wheel.

```bash
pip install viva-genicam
```

The install also provides `viva-camctl`, the diagnostic CLI, so
`viva-camctl report --ip <CAMERA-IP> --out viva-report.txt` works without a Rust
toolchain. It is the right thing to attach to a bug report.

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
cam = vg.connect_gige(cams[0])

print(cam.get("DeviceModelName"))
cam.set_exposure_time_us(10_000.0)

with cam.stream() as frames:
    for frame in frames:
        arr = frame.to_numpy()          # shape and dtype follow the pixel format
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

## Licensing

The `viva-genicam` code is MIT-licensed. The binary wheel additionally
statically links [libusb](https://libusb.info) (LGPL-2.1-or-later) for
USB3 Vision support, so the package license is
`MIT AND LGPL-2.1-or-later`. See `THIRD-PARTY-NOTICES.md` (shipped in the
wheel and sdist) for details, including how to rebuild the native module
against a modified or system libusb per LGPL §6.
