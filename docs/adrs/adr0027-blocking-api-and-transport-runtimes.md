# ADR-0027: The Blocking API, and a GigE Transport That Can Own Its Runtime

**Status:** Accepted
**Date:** 2026-10-11

## Context

[ADR-0014](adr0014-sync-registerio-async-adapters.md) made `RegisterIo`
synchronous and bridged the async `GigeDevice` with `block_in_place` +
`Handle::block_on` on the runtime the camera was opened from. It recorded
the cost: a current-thread runtime panics. That was not hypothetical —
`#[tokio::main(flavor = "current_thread")]` and `#[tokio::test]` both make
one — and it was deeper than the panic. A current-thread runtime drives its
sockets only while its one thread is inside `block_on`, so a register read
waiting on that thread can never be answered: removing the panic alone would
have turned it into a hang.

Backlog API-16 also asked for programs with no runtime at all: open,
discover, get/set, execute, stream and frames. Most of that was synchronous
already — feature access, `Camera::open_stream`, `FrameStream::next_frame_blocking`
— and needed only a runtime to exist somewhere. Connecting and discovery were
async; so was waiting for a command to finish.

The two shapes on offer were a `blocking` module owning a runtime (the
`reqwest::blocking` pattern), or making the existing synchronous paths work
from any context.

## Decision

Both, sharing one mechanism: **a GigE transport may own the runtime its
socket lives on.**

1. `GigeRegisterIo` bridges to its runtime from any context: a plain thread
   blocks on the runtime directly; a multi-thread runtime's worker goes
   through `block_in_place`; a current-thread runtime's thread hands the
   wait to a scoped thread. The one case that cannot work — a transport on
   a current-thread runtime, used from that runtime's own thread — keeps
   Tokio's refusal, a panic, rather than becoming a hang; only a transport
   built by hand with `GigeRegisterIo::new` can be in that state, and it
   still works from the runtime's blocking pool, as it did before.
2. `connect_gige` / `connect_gige_as` called from a current-thread runtime
   open the camera on a runtime of its own: one worker thread that drives the
   socket, the keepalive and their timers. The transport owns it and shuts it
   down in the background after releasing control on drop. From a
   multi-thread runtime nothing changes: the camera uses the caller's.
3. `viva_genicam::blocking` gives a program without a runtime `discover`,
   `discover_with`, `discover_ip`, `connect` and `connect_as`. `connect`
   builds the same one-worker runtime. The camera it returns is the ordinary
   `Camera<GigeRegisterIo>` — there is no second camera type — so every
   synchronous method, the standard-feature helpers among them, works
   unchanged. The async methods that only wait gained `_blocking` twins
   (`Camera::execute_and_wait_blocking`), following
   `FrameStream::next_frame_blocking`. The module refuses to run inside a
   runtime, which it would stall.

## Consequences

### Positive

- One camera type and one set of methods for async and blocking callers;
  the blocking module is five functions, not a parallel API.
- No camera opened through `connect_gige` or `blocking` can panic or hang
  on a register access whatever runtime the caller runs, and the documented
  "the runtime must outlive the transport" rule holds by construction for a
  camera that owns its runtime.
- The services, Studio and Python, which already call the synchronous
  methods from the blocking pool, are unaffected.

### Negative

- A camera opened from a current-thread runtime, or through `blocking`,
  costs a thread (plus Tokio's blocking pool, idle until the keepalive
  uses it) for as long as it lives.
- From a current-thread runtime's thread each register access spawns a
  scoped thread. Correct, but slower than the multi-thread path; the
  multi-thread runtime remains the recommended one.
- Futures run through the bridge must be `Send`, so code that held the
  device's `std::sync::MutexGuard` inside a future now locks it outside and
  lends the future `&mut GigeDevice`.
- USB3 Vision is only half covered: opening a camera is synchronous, but
  `U3vFrameStream` still reads on a Tokio runtime's blocking pool
  (backlog API-11).
