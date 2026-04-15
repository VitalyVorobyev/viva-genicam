"""Capture N frames from the first discovered camera and save them as PNGs.

Demonstrates:
  - `camera.stream()` as a context manager that starts/stops acquisition
  - iterating over frames and calling `frame.to_numpy()`
  - writing an (H, W) or (H, W, 3) uint8 array to disk

Requires `Pillow` for PNG writing:
    pip install Pillow

Usage:
    python grab_frame.py              # save 1 frame as frame_001.png
    python grab_frame.py --count 5    # save 5 frames
    python grab_frame.py --rgb         # force RGB output for mono cameras
    python grab_frame.py --iface en0   # override the GigE NIC
"""

from __future__ import annotations

import argparse
import sys

import viva_genicam as vg


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--count", type=int, default=1)
    p.add_argument("--rgb", action="store_true", help="always save as RGB even for mono")
    p.add_argument("--iface", default=None, help="NIC override for GigE streaming")
    p.add_argument("--prefix", default="frame", help="output filename prefix")
    args = p.parse_args()

    try:
        from PIL import Image
    except ImportError:
        print("Pillow is required: pip install Pillow", file=sys.stderr)
        sys.exit(1)

    cams = vg.discover(timeout_ms=1000, all=True)
    if not cams:
        raise SystemExit("no GigE cameras found")
    cam = vg.connect_gige(cams[0], iface=args.iface)
    print(f"Connected to {cams[0].model or '(unknown)'} @ {cams[0].ip}")

    saved = 0
    with cam.stream(auto_packet_size=False) as frames:
        for frame in frames:
            saved += 1
            arr = frame.to_rgb8() if args.rgb else frame.to_numpy()
            path = f"{args.prefix}_{saved:03d}.png"
            Image.fromarray(arr).save(path)
            print(
                f"  {path}: {frame.width}x{frame.height} {frame.pixel_format} "
                f"({len(frame.payload())} bytes)"
            )
            if saved >= args.count:
                break


if __name__ == "__main__":
    main()
