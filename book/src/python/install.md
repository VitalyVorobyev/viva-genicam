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

The repo ships a fake GigE Vision camera that binds to loopback. Build it once, then run the end-to-end demo:

```bash
git clone https://github.com/VitalyVorobyev/viva-genicam
cd viva-genicam
cargo build -p viva-fake-gige --release
python crates/viva-pygenicam/examples/demo_fake_camera.py
```

Expected output:

```
1. Starting fake GigE camera on 127.0.0.1:3956 ...
2. Discovering ...
   found FakeGigE @ 127.0.0.1
3. Connecting ...
   connected; XML is 16115 bytes, 53 features
4. Reading features:
   Width          = 800
   Height         = 480
   ...
6. Streaming 5 frames ...
   frame 1: 800x480 Mono8  numpy shape=(480, 800) dtype=uint8
   ...
Demo complete — everything ran without any real hardware.
```

This covers discovery, connection, feature read/write, and streaming — the full surface you will use with a real camera.

## Next

→ [Discovery](discovery.md) — enumerate cameras with interface control.
