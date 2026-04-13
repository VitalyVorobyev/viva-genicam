"""Walk the camera's GenICam NodeMap and print it as a tree.

Demonstrates:
  - `camera.categories()` -- category -> feature-name mapping
  - `camera.node_info(name)` -- kind / access / visibility / description
  - filtering by kind or access mode

Usage:
    python node_browser.py
    python node_browser.py --kind Enumeration     # only enums
    python node_browser.py --writable              # only RW/WO features
    python node_browser.py --visibility Beginner   # hide Expert/Guru
"""

from __future__ import annotations

import argparse

import viva_genicam as vg


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--kind", default=None, help='e.g. "Integer", "Float", "Enumeration"')
    p.add_argument("--writable", action="store_true")
    p.add_argument("--visibility", default=None, help='"Beginner", "Expert", or "Guru"')
    args = p.parse_args()

    cams = vg.discover(timeout_ms=1000, all=True)
    if not cams:
        raise SystemExit("no GigE cameras found")
    cam = vg.connect_gige(cams[0])
    print(f"Connected to {cams[0].model or '(unknown)'} @ {cams[0].ip}\n")

    def wanted(info: vg.NodeInfo) -> bool:
        if args.kind and info.kind != args.kind:
            return False
        if args.writable and not info.writable:
            return False
        if args.visibility and info.visibility != args.visibility:
            return False
        return True

    categories = cam.categories()
    printed = 0
    for cat_name, children in categories.items():
        cat_infos = [cam.node_info(c) for c in children]
        cat_infos = [i for i in cat_infos if i is not None and wanted(i)]
        if not cat_infos:
            continue
        print(f"[{cat_name}]")
        for info in cat_infos:
            display = info.display_name or info.name
            line = f"  {display:<28} {info.kind:<12} {info.access or '--':<3}  {info.visibility}"
            if info.description:
                line += f"  — {info.description[:60]}"
            print(line)
            printed += 1
        print()

    print(f"{printed} feature(s) shown out of {len(cam.nodes())} total")


if __name__ == "__main__":
    main()
