# ADR-0015: Vendored libusb in PyPI Wheels with LGPL Notices

**Status:** Superseded by [ADR-0032](adr0032-usb3-vision-transport.md)

> **What changed (0.9):** USB3 Vision moved from `rusb` to `nusb`, which is
> pure Rust. No libusb is built or linked, the Python package is MIT, and the
> notices this ADR added are gone.
**Date:** 2026-07-25

## Context

The Python wheels (`viva-genicam` on PyPI) statically link a vendored
libusb 1.0.27 via `rusb`'s `vendored` feature so that `pip install` works
with no system dependencies. libusb is LGPL-2.1-or-later, but the package
metadata claimed plain MIT and no notices shipped — an LGPL §6 compliance
gap: users received LGPL object code without the license text, attribution,
or a relink path.

Options:

- **(a) Link system libusb dynamically.** Clean licensing (LGPL's dynamic
  path), but breaks "pip install just works": users would need libusb dev
  packages per platform, and wheel builds would depend on runner state.
- **(b) Keep the vendored static link and ship full LGPL compliance
  material.** §6 permits static linking when license text, notices, and a
  relink route are provided.

## Decision

Option (b), shipped in 0.2.6:

- Wheel/sdist license metadata is `MIT AND LGPL-2.1-or-later`.
- `THIRD-PARTY-NOTICES.md` plus the full LGPL-2.1 text ship inside the
  wheel and sdist.
- The §6 relink route is documented (rebuild the extension against a system
  libusb; the object layout permits relinking).

The crates.io and prebuilt-binary path links system libusb *dynamically* and
is unaffected — this ADR concerns only the PyPI wheels.

## Consequences

**Positive:** `pip install viva-genicam` stays dependency-free on every
platform while being LGPL-compliant; the licensing story is explicit instead
of accidentally wrong.

**Negative:** dual-license metadata can surprise MIT-only consumers scanning
dependencies, and we own keeping the vendored libusb version, its notices,
and the metadata in sync on every bump. cargo-deny's license gate
(ADR-0016) now watches for regressions.
