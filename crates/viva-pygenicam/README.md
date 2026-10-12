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
with vg.connect_gige(cams[0]) as cam:   # control is given back at the end
    print(cam.get("DeviceModelName"))
    cam.set_exposure_time(10_000.0)

    with cam.stream() as frames:
        for frame in frames:
            arr = frame.to_numpy()      # (H, W) uint16 for a 12-bit camera, ...
            print(frame.width, frame.height, frame.pixel_format)
            break
```

Cameras and frames work from any thread, and calls that wait on the camera
release the GIL. `viva_genicam.testing.FakeGigeCamera` is a camera in the same
process, for tests and demos without hardware.

See the [documentation](https://vitalyvorobyev.github.io/viva-genicam/python.html) for the full API.

## Build from source

See [CONTRIBUTING.md](https://github.com/VitalyVorobyev/viva-genicam/blob/main/CONTRIBUTING.md)
in the repository.

## Licensing

The `viva-genicam` code is MIT-licensed. The binary wheel additionally
statically links [libusb](https://libusb.info) (LGPL-2.1-or-later) for
USB3 Vision support, so the package license is
`MIT AND LGPL-2.1-or-later`. See `THIRD-PARTY-NOTICES.md` (shipped in the
wheel and sdist) for details, including how to rebuild the native module
against a modified or system libusb per LGPL §6.
