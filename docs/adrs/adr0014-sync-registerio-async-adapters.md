# ADR-0014: Synchronous RegisterIo with Async Adapters

**Status:** Accepted
**Date:** 2026-07-25

## Context

Every GenApi feature read/write bottoms out in register I/O. The transports
(UDP sockets, USB bulk) are naturally async in tokio, so the obvious choice
was an async `RegisterIo` trait. But GenApi node evaluation is deeply
recursive and synchronous by nature: reading one feature may walk pValue
delegation, selectors, SwissKnife expressions, and converters, each step
possibly touching registers. An async trait would make every node-evaluation
function async, force trait-object overhead (boxed futures) on the hot path,
and poison the engine for consumers with no runtime at all — wasm builds and
plain sync CLI code.

## Decision

`RegisterIo` is a synchronous trait. Async transports are wrapped by
adapters: `GigeRegisterIo` bridges to the async `GigeDevice` using
`tokio::task::block_in_place` + `Handle::block_on`, which is safe to call
from both async and sync contexts. `MockIo` (tests) and `NullIo` (offline
XML browsing) implement the trait directly with no runtime involved.

## Consequences

**Positive:**

- The NodeMap engine stays plain recursive Rust — no viral `async`, no boxed
  futures per register access, straightforward to reason about and test.
- `viva-genapi` works on `wasm32-unknown-unknown` and in sync binaries
  because the trait itself demands no runtime.
- Test doubles are trivial (implement one sync trait).

**Negative:**

- The async boundary needs care: calling a blocking bridge on a runtime
  worker thread must not stall the executor. `block_in_place` handles the
  direct case; the services additionally wrap camera access in
  `spawn_blocking` (`DeviceHandle`) so Zenoh handlers never block.
- `block_in_place` requires the multi-threaded tokio runtime; a
  current-thread runtime would panic. This constraint is on the adapter,
  not the trait — an async-native adapter could be added later without
  touching the engine.

## Amendment — 2026-10-11

The adapter no longer panics on a current-thread runtime: it bridges from any
context, and a camera opened from a current-thread runtime, or through the
blocking API, gets a runtime of its own
([ADR-0027](adr0027-blocking-api-and-transport-runtimes.md)). The trait is
unchanged.
