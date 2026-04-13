"""Self-contained end-to-end demo — no hardware required.

Spawns the `viva-fake-gige` binary on loopback, discovers it, reads and
writes features, and streams 5 frames. Mirrors the Rust
`demo_fake_camera` example.

Prerequisites:
    cargo build -p viva-fake-gige --release

Usage:
    python demo_fake_camera.py
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path

import viva_genicam as vg

REPO_ROOT = Path(__file__).resolve().parents[3]
FAKE_BIN = REPO_ROOT / "target" / "release" / "viva-fake-gige"


def wait_for_discovery(deadline: float) -> vg.GigeDeviceInfo:
    """Poll discovery until the fake camera responds."""
    while time.time() < deadline:
        cams = vg.discover(timeout_ms=300, all=True)
        for c in cams:
            if c.ip.startswith("127."):
                return c
        time.sleep(0.1)
    raise RuntimeError("fake camera did not come up in time")


def main() -> None:
    if not FAKE_BIN.exists():
        print(
            f"Missing {FAKE_BIN} — build it first:\n"
            "    cargo build -p viva-fake-gige --release",
            file=sys.stderr,
        )
        sys.exit(1)

    env = os.environ.copy()
    env.setdefault("RUST_LOG", "warn")

    print("1. Starting fake GigE camera on 127.0.0.1:3956 ...")
    proc = subprocess.Popen(
        [str(FAKE_BIN), "--bind", "127.0.0.1", "--port", "3956"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        print("2. Discovering ...")
        info = wait_for_discovery(time.time() + 10.0)
        print(f"   found {info.model or '(unknown)'} @ {info.ip}")

        print("3. Connecting ...")
        cam = vg.connect_gige(info)
        print(
            f"   connected; XML is {len(cam.xml)} bytes, "
            f"{len(cam.nodes())} features"
        )

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
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    main()
