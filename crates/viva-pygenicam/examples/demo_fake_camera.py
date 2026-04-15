"""Self-contained end-to-end demo — no hardware, no repo clone.

Everything runs in-process: the fake camera is shipped inside the wheel.

Prerequisites: `pip install viva-genicam` (or a local wheel build).

Usage:
    python demo_fake_camera.py
"""

from __future__ import annotations

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera


def main() -> None:
    print("1. Starting in-process fake GigE camera ...")
    with FakeGigeCamera(width=640, height=480, fps=10) as fake:
        print(f"   bound to {fake.ip}:{fake.port}")

        print("2. Discovering ...")
        info = fake.device_info()
        print(f"   found {info.model or '(unknown)'} @ {info.ip}")

        print("3. Connecting ...")
        cam = vg.connect_gige(info)
        print(f"   connected; XML is {len(cam.xml)} bytes, {len(cam.nodes())} features")

        print("4. Reading features:")
        for name in ("Width", "Height", "PixelFormat", "ExposureTime", "Gain"):
            try:
                print(f"   {name:<14} = {cam.get(name)}")
            except vg.GenicamError as exc:
                print(f"   {name:<14} ERROR: {exc}")

        print("5. Writing ExposureTime = 7500 µs ...")
        cam.set_exposure_time_us(7500.0)
        print(f"   readback: {cam.get('ExposureTime')}")

        print("6. Streaming 5 frames ...")
        with cam.stream(auto_packet_size=False) as frames:
            for i, frame in enumerate(frames, 1):
                arr = frame.to_numpy()
                print(
                    f"   frame {i}: {frame.width}x{frame.height} "
                    f"{frame.pixel_format}  numpy shape={arr.shape} dtype={arr.dtype}"
                )
                if i >= 5:
                    break

    print("\nDemo complete — everything ran without any real hardware.")


if __name__ == "__main__":
    main()
