"""Execute a GenApi ``<Command>`` feature — the "reset the camera" workflow.

`<Command>` features are actions rather than values: ``UserSetLoad``,
``TimestampLatch``, ``TriggerSoftware``. ``camera.execute(name)`` runs one.

This is the flow from issue #121: select a user set, then load it.

    cam.set("UserSetSelector", "Default")
    cam.execute("UserSetLoad")

``execute`` writes the value the camera's XML names for the command and waits
until the camera reports it finished (GenICam's ``IsDone``), for up to
``timeout`` seconds; ``wait=False`` returns once the write is acknowledged.

``cam.set("UserSetLoad", "1")`` also executes the command, without waiting —
``set`` dispatches Command nodes and discards the value. It reads like a write
and the value is meaningless, so prefer ``execute``.

Usage:
    python execute_command.py                          # list Command features
    python execute_command.py --name UserSetLoad       # execute one
    python execute_command.py --reset                  # UserSetSelector + Load
"""

from __future__ import annotations

import argparse
import sys

import viva_genicam as vg


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--name", help="command feature to execute")
    p.add_argument(
        "--reset",
        action="store_true",
        help="select the Default user set and load it",
    )
    p.add_argument("--user-set", default="Default", help="user set for --reset")
    args = p.parse_args()

    cameras = vg.discover(timeout_ms=3000)
    if not cameras:
        print("No GigE cameras found.", file=sys.stderr)
        raise SystemExit(1)

    cam = vg.connect_gige(cameras[0])
    print(f"Connected to {cameras[0].ip}")

    if args.reset:
        entries = cam.enum_entries("UserSetSelector")
        if args.user_set not in entries:
            print(
                f"{args.user_set!r} is not one of {entries}",
                file=sys.stderr,
            )
            raise SystemExit(1)
        cam.set("UserSetSelector", args.user_set)
        cam.execute("UserSetLoad")
        print(f"Loaded user set {args.user_set!r}")
        return

    if args.name:
        info = cam.node_info(args.name)
        if info is None:
            print(f"{args.name!r} is not a feature on this camera", file=sys.stderr)
            raise SystemExit(1)
        if info.kind != "Command":
            print(
                f"{args.name!r} is a {info.kind}, not a Command — "
                "use camera.set() for it",
                file=sys.stderr,
            )
            raise SystemExit(1)
        cam.execute(args.name)
        print(f"Executed {args.name}")
        return

    # Default: list what this camera can be told to do.
    commands = [i.name for i in cam.all_node_info() if i.kind == "Command"]
    print(f"{len(commands)} Command features:")
    for name in sorted(commands):
        print(f"  {name}")


if __name__ == "__main__":
    main()
