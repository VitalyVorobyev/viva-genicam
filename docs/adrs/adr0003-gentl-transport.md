# ADR-0003: GenTL as sole transport abstraction

**Status:** Superseded by [ADR-0011](adr0011-pure-rust-genicam-stack.md)
**Date:** 2026-03-06

> **What changed.** This decision was reversed outright: GenTL appears nowhere in
> `crates/`, and the transports are implemented from the EMVA specifications in
> `viva-gige` (GVCP/GVSP) and `viva-u3v`. ADR-0011 has the argument. The cost of
> the reversal is the one this ADR's consequences list did not anticipate — we
> now own conformance against real hardware ourselves, which is what ADR-0018,
> ADR-0019 and the vendor XML corpus exist to manage. The benefit is that a user
> needs no vendor `.cti` provider installed, and that the stack cross-compiles.
> CoaXPress and CameraLink, which GenTL would have covered for free, are
> consequently not supported.

## Context

Industrial cameras use various transport layers: GigE Vision, USB3 Vision, CoaXPress, CameraLink. Each has vendor-specific SDKs. GenTL (GenICam Transport Layer) is the standardized C API that abstracts all these transports behind a single interface.

## Decision

The camera service uses **GenTL exclusively** as its transport abstraction.

- The service dynamically loads GenTL provider `.cti` files at runtime.
- No direct dependency on Aravis, vendor SDKs, or transport-specific libraries.
- The Rust library wraps the GenTL C API using `libloading` for dynamic loading.
- The service discovers available `.cti` providers on the system and enumerates cameras through them.

## Consequences

- Single abstraction covers GigE Vision, USB3 Vision, CoaXPress, and CameraLink.
- Users must have a GenTL provider installed (most camera vendors ship one).
- No compile-time dependency on camera vendor SDKs.
- Provider discovery path is OS-dependent (`GENICAM_GENTL*_PATH` environment variables).
- Some advanced vendor-specific features may not be accessible through GenTL alone.
