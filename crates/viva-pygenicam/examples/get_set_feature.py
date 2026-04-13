"""Connect to the first discovered GigE camera and read/write features.

Demonstrates:
  - `camera.get(name)` -- read any feature as a string
  - `camera.set(name, value)` -- set from a string (parsed per node type)
  - typed helpers `set_exposure_time_us` / `set_gain_db`
  - `camera.node_info(name)` -- introspection
  - `camera.enum_entries(name)` -- allowed values for enum features

Usage:
    python get_set_feature.py                  # uses first camera found
    python get_set_feature.py --exposure 7500  # override exposure (µs)
"""

from __future__ import annotations

import argparse

import viva_genicam as vg


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--exposure", type=float, default=None, help="exposure time in µs")
    p.add_argument("--gain", type=float, default=None, help="gain in dB")
    args = p.parse_args()

    cams = vg.discover(timeout_ms=1000, all=True)
    if not cams:
        raise SystemExit("no GigE cameras found")
    cam = vg.connect_gige(cams[0])
    print(f"Connected to {cams[0].model or '(unknown)'} @ {cams[0].ip}")

    for feature in ("DeviceModelName", "Width", "Height", "PixelFormat", "ExposureTime", "Gain"):
        info = cam.node_info(feature)
        if info is None:
            print(f"  {feature}: <not present>")
            continue
        try:
            value = cam.get(feature)
        except vg.GenicamError as exc:
            value = f"<error: {exc}>"
        print(f"  {feature:<18} [{info.kind:<11} {info.access or '--'}]  = {value}")

    # Enum introspection (pixel format entries).
    try:
        entries = cam.enum_entries("PixelFormat")
        print(f"\nPixelFormat entries: {entries}")
    except vg.GenicamError:
        pass

    if args.exposure is not None:
        print(f"\nSetting ExposureTime = {args.exposure} µs ...")
        cam.set_exposure_time_us(args.exposure)
        print(f"  readback: {cam.get('ExposureTime')}")

    if args.gain is not None:
        print(f"\nSetting Gain = {args.gain} dB ...")
        cam.set_gain_db(args.gain)
        print(f"  readback: {cam.get('Gain')}")


if __name__ == "__main__":
    main()
