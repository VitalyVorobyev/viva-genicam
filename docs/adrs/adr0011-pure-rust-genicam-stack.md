# ADR-0011: Pure-Rust GenICam Stack

**Status:** Accepted
**Date:** 2026-07-25

## Context

A mature C implementation of the GenICam ecosystem exists: aravis (LGPL).
Binding it or porting it would have delivered a working camera stack far
faster than implementing GVCP/GVSP, GenCP, U3V, and a GenApi engine from
the EMVA specifications. The alternative — a from-scratch Rust
implementation — costs years of conformance work against real hardware.

## Decision

Implement the entire stack in pure Rust from the published specifications.
Do not bind, port, or copy aravis code. Keep aravis (`../aravis`) only as an
optional third-party conformance reference for comparative testing; it is
never required for development or CI.

Reasons:

- **Dependency licensing.** Avoiding an LGPL implementation removes its
  linking and redistribution obligations from the core path, including static
  binaries and wasm. This does not resolve third-party rights in standards,
  specifications or trademarks; see the [licensing review](../legal-review.md).
- **Memory safety on untrusted input.** GVCP/GVSP packets and camera XML
  arrive from the network; parsing them in safe Rust removes an entire bug
  class where a hostile or broken device corrupts host memory.
- **No C toolchain for consumers.** `cargo add viva-genicam` and
  `pip install viva-genicam` must work without system dev packages.
- **Full control of the async story.** Bolting async Rust onto a
  GLib-main-loop C library produces impedance mismatch everywhere; owning the
  stack lets the sync/async boundary sit where we choose (see ADR-0014).

## Consequences

**Positive:** MIT-licensed core, static linking and wasm targets unblocked,
safe parsers on the network boundary, one language across the stack.

**Licensing clarification (2026-10-25):** independent implementation avoids
inheriting a copied implementation's license; it does not establish unrestricted
rights to standards, trademarks, or third-party intellectual property. MIT
covers our code. See the [licensing and provenance review](../legal-review.md)
for unresolved A3 distribution questions and separate dependency obligations.

**Negative:** We must earn spec conformance ourselves. The mitigation is the
fake-camera-first strategy plus a real-hardware feedback loop (ADR-0013) —
and the two 2026 incidents (SCPS, READMEM alignment) show that loop is not
optional. Conformance testing against aravis remains a useful cross-check
precisely because it is an independent implementation.

**2026-10-25 clarification:** this decision chooses an implementation and
dependency architecture. It does not establish a clean-room history, A3
permission, patent clearance, or permission to redistribute specification text.
