# ADR-0023: Heartbeats during GVCP acknowledgement waits

**Status:** Proposed
**Date:** 2026-10-09

## Context

`GigeRegisterIo` serializes normal GVCP operations and its idle keepalive on
one `Mutex<GigeDevice>`. A read owns that mutex until its ACK arrives. A long
wait, including an extension requested by PENDING_ACK, can therefore prevent
the keepalive from sending before the camera's heartbeat window expires.

An ephemeral UDP regression fixture advertises a 300 ms heartbeat window and
delays an operation ACK by 900 ms. Without in-flight heartbeats, the following
write fails because control has expired. With probes enabled, the delayed
operation and following write succeed. A PENDING_ACK variant reproduces the
same starvation with the library's stock 500 ms initial ACK timeout.

## Decision

Keep ordinary transactions serialized. Give the single GVCP socket reader
responsibility for refreshing the heartbeat during an active transaction's
wait. It sends an idempotent CCP read on the same source socket with a separate
request ID, and filters unrelated ACKs before returning the active request's
ACK. Memory-only sessions send READMEM instead of READREG.

The idle background task remains in `viva-genicam`. It configures the in-flight
period from the device heartbeat window. Raw `GigeDevice` users opt in, so
constructing a low-level handle alone does not start a keepalive. The ACK timeout
is configurable per attempt and retains its existing 500 ms default.

Heartbeat replies never restart the active ACK deadline. Only a matching
PENDING_ACK grants the original operation additional time. Request-ID allocation
skips the active transaction's ID if the 16-bit counter wraps.

## Consequences

### Positive

- Long ACK waits can refresh the heartbeat without changing camera settings.
- Heartbeat traffic never repeats the active write or file operation.
- One reader owns the socket; there are no competing consumers of ACKs.
- Idle keepalive and normal operation ordering retain their current design.

### Negative

- This is not a general concurrent request dispatcher. Caller code that holds
  the device mutex without performing I/O can still block the idle task.
- Cameras must process CCP reads while another operation is pending. The
  regression fixture exercises this behavior; the transport fix also needs
  hardware testing before assuming it works on every device.
- Probes run only inside a receive wait that some caller is already blocked
  on, so they need no shutdown of their own: dropping `GigeRegisterIo`
  cancels the idle task's wait (#162), and with it any probe it would send.

## Amendment — 2026-10-11 (milestone L)

- A heartbeat that cannot be sent is logged and the wait goes on; it no
  longer ends the wait as a receive error, which had the active command sent
  again (TC-26). An ICMP error on the socket is ignored likewise once the
  device has answered `PENDING_ACK` or a heartbeat has been sent in the wait —
  it may be the heartbeat's, and the command may be executing. Before either,
  it still ends the wait, so an absent device fails fast.
- A `GevHeartbeatTimeout` of 0 no longer switches the in-flight probes off;
  like the idle keepalive they assume a 3 000 ms window.
- The regression fixture for the facade moved into `viva-fake-gige`
  (`delay_ack`, `pending_ack`, `enforce_heartbeat`), so the test ships with
  the package that runs it.
