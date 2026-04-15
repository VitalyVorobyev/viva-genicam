# Install & hello-camera

## Install from PyPI

```bash
pip install viva-genicam
```

Wheels ship for:

| OS | Arch | Python |
|---|---|---|
| Linux (manylinux_2_28) | x86_64 | 3.9+ (abi3) |
| macOS | arm64 | 3.9+ (abi3) |
| Windows | x86_64 | 3.9+ (abi3) |

libusb is statically linked into the extension module — no need to `apt install libusb-1.0-0-dev` or `brew install libusb` on the install side.

If you are on a platform without a pre-built wheel, `pip` falls back to the sdist; you will need a Rust toolchain (`rustup`) and a C compiler installed.

## Verify the install

```python
import viva_genicam as vg
print(vg.__version__)
print(vg.discover(timeout_ms=300))
```

If no cameras are physically connected, you should see an empty list — not an exception.

## Hello camera — no hardware needed

The wheel ships an in-process fake GigE Vision camera. Just run:

```python
import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera

with FakeGigeCamera(width=640, height=480, fps=10) as fake:
    cam = vg.connect_gige(fake.device_info())
    print(cam.get("DeviceModelName"))
    with cam.stream() as frames:
        frame = frames.next_frame(timeout_ms=5000)
        print(frame.width, frame.height, frame.pixel_format)
```

No clone, no `cargo build`, no subprocess — the fake camera lives inside the same process as your script.

For a fuller end-to-end walkthrough, the repo ships a runnable example:

```bash
python crates/viva-pygenicam/examples/demo_fake_camera.py
```

Expected output:

```
1. Starting in-process fake GigE camera ...
   bound to 127.0.0.1:3956
2. Discovering ...
   found FakeGigE @ 127.0.0.1
3. Connecting ...
   connected; XML is 16115 bytes, 53 features
4. Reading features:
   Width          = 640
   Height         = 480
   ...
6. Streaming 5 frames ...
   frame 1: 640x480 Mono8  numpy shape=(480, 640) dtype=uint8
   ...
Demo complete — everything ran without any real hardware.
```

This covers discovery, connection, feature read/write, and streaming — the full surface you will use with a real camera.

## Next

→ [Discovery](discovery.md) — enumerate cameras with interface control.
