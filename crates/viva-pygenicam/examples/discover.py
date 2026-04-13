"""List every GigE Vision and USB3 Vision camera this host can see.

Usage:
    python discover.py                    # GigE on the default NIC
    python discover.py --all               # GigE across every local interface
    python discover.py --iface en0         # GigE on one named NIC
    python discover.py --timeout-ms 2000   # slower cameras take longer to reply
"""

from __future__ import annotations

import argparse

import viva_genicam as vg


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--timeout-ms", type=int, default=500)
    p.add_argument("--iface", default=None, help="restrict GigE discovery to one NIC")
    p.add_argument("--all", action="store_true", help="scan every local NIC")
    args = p.parse_args()

    print(f"GigE Vision (timeout={args.timeout_ms} ms):")
    gige = vg.discover(timeout_ms=args.timeout_ms, iface=args.iface, all=args.all)
    if not gige:
        print("  (no cameras found)")
    for c in gige:
        print(
            f"  {c.ip:<15}  mac={c.mac}  "
            f"model={c.model or '-'}  mfr={c.manufacturer or '-'}"
        )

    print("\nUSB3 Vision:")
    u3v = vg.discover_u3v()
    if not u3v:
        print("  (no cameras found)")
    for c in u3v:
        print(
            f"  vid:pid=0x{c.vendor_id:04x}:0x{c.product_id:04x}  "
            f"bus={c.bus} addr={c.address}  model={c.model or '-'}  serial={c.serial or '-'}"
        )


if __name__ == "__main__":
    main()
