# ADR-0020: Per-Transport Status Codes over a Shared Table

**Status:** Proposed
**Date:** 2026-07-30

## Context

`viva-gencp` defines one `StatusCode` enum, and both transports decode
their acknowledgement status through it: `viva-gige` (GVCP) via
`GigeError::Status`, and `viva-u3v` (GenCP over USB) via `decode_ack`.
The enum has six named variants plus `Unknown(u16)`.

Three of those six are wrong, and the shape of the enum is why the errors
went unnoticed for so long.

**The immediate defect.** A user on [#45] wrote an exposure value to a
FLIR BFS-PGE-31S4C and got:

```
io error: device reported status Unknown(32774)
```

`32774` is `0x8006`, `ACCESS_DENIED`. Checking the table against Wireshark's
GVCP dissector (`epan/dissectors/packet-gvcp.c:204-229`) and aravis
(`arvgvcpprivate.h`, `arvuvcpprivate.h`) — two implementations that agree
with each other completely — found:

| raw | our name | actual |
|---|---|---|
| 0x8004 | `DeviceBusy` | `WRITE_PROTECT` |
| 0x8005 | `Error` | `BAD_ALIGNMENT` |
| 0x8006 | *(none)* | `ACCESS_DENIED` |
| 0x8007 | *(none)* | `BUSY` |
| 0x8FFF | *(none)* | `ERROR` (the generic one) |

So a write to a write-protected register reports "device busy", and the
generic error — the one a device sends when it has nothing more specific —
has no name at all.

**Why adding rows is not enough.** The two tables are not the same table.
They share `0x0000`, `0x8001`–`0x8007` and `0x8FFF`, and then diverge:

- GVCP owns the packet-resend family: `0x0100` `PACKET_RESEND`, and
  `0x800C`–`0x8017` (`PACKET_UNAVAILABLE`, `DATA_OVERRUN`,
  `PACKET_NOT_YET_AVAILABLE`, `NO_REF_TIME`, `OVERFLOW`, `ACTION_LATE`,
  `LEADER_TRAILER_OVERFLOW`, …).
- GenCP owns `0xA001`–`0xA101` (`RESEND_NOT_SUPPORTED`,
  `DSI_ENDPOINT_HALTED`, `SI_PAYLOAD_SIZE_NOT_ALIGNED`,
  `SI_REGISTERS_INCONSISTENT`, `DATA_DISCARDED`, `DATA_OVERRUN`).

And they **conflict at the same code**:

| raw | GVCP | GenCP |
|---|---|---|
| `0x800B` | `NO_MSG` (deprecated) | `MSG_TIMEOUT` |

A single flat enum cannot decode `0x800B` correctly for both transports, so
no amount of added variants fixes it. This is the decision-forcing fact.

**What the shared table already cost us.** `viva-u3v` needed to recognise
GenCP's pending-acknowledge, found no variant for it, and reached for
`StatusCode::Unknown(0x8006)` (`control.rs:29`, `:189`) — attaching the
meaning "device needs more time" to what is really `ACCESS_DENIED`. The
belief was then recorded in the backlog as settled (TC-01: "U3V signals the
same condition with status `0x8006`, so the two transports do not share a
representation") and asserted in a unit test (`control.rs:472`).

Pending-acknowledge is not a status in either protocol. GVCP models it as
an opcode (`0x0089`) and we already had that right. GenCP models it the
same way, as command id `0x0805`. The consequences of the misreading:

- A genuine `ACCESS_DENIED` on U3V is slept-and-retried 100 times
  (`MAX_PENDING_RETRIES`) and then reported as `Timeout`.
- A genuine pending-ack is never detected as one. Worse, `decode_ack`
  discards the acknowledgement's command id (`let _opcode`, `:267`) and
  `transact` validates only the request id, so the pending-ack is accepted
  as the real answer and its `time_to_completion` SCD is returned **as the
  register value**. A 4-byte read gets a plausible wrong number.

That is the fourth occurrence of the pattern ADR-0019 names: a fake or a
test encoding the client's own wrong assumption, so the round trip proves
nothing.

## Decision

**Give each transport its own status type, over a shared core.**

1. `viva-gencp` keeps a `StatusCode` for the codes the two protocols
   genuinely share (`0x0000`, `0x8001`–`0x8007`, `0x8FFF`), with the
   corrected meanings. It gains no transport-specific variant.
2. `viva-gige` and `viva-u3v` each own an enum that wraps the shared core
   and adds its own range. Decoding is per transport, so `0x800B` resolves
   correctly on both.
3. Every status type carries a `Display` that prints **the name and the raw
   hex together**. `Unknown(32774)` is what sent a user to the issue
   tracker; `ACCESS_DENIED (0x8006)` would not have. Unknown codes print as
   hex, never bare decimal.
4. Pending-acknowledge is modelled as a **command id** in both transports,
   never as a status. `viva-u3v`'s `decode_ack` surfaces the command id,
   and `transact` requires it to equal `OpCode::ack_code()` before
   accepting a payload — the check aravis makes at
   `arvuvdevice.c:344-373`.
5. Fixtures are spec-derived per ADR-0019: literal byte arrays for each
   acknowledgement shape, asserted independently of our own encoder.

**Evidence standing.** The tables above come from two corroborating
implementations plus one hardware data point (#45's `0x8006`), *not* from
the specification text, which we do not have locally. Per the evidence
hierarchy that is strong enough to correct a clear defect and to choose the
structure, but the individual code meanings must be checked against GigE
Vision §18.4 and the GenCP status table before this ADR moves to Accepted.

### What is implemented so far

Points 3, 4 and 5 landed with this ADR, together with the corrected shared
`StatusCode`. Points 1 and 2 — the per-transport types — are backlog **TC-16**,
which tracks what remains ([`docs/backlog.md`](../backlog.md)).

## Consequences

### Positive

- `0x800B` becomes decodable at all, and three mislabelled codes start
  telling the truth. A write-protected register stops reporting "busy".
- Device errors become readable without a hex converter, which is the
  difference between a user filing a useful report and filing
  `Unknown(32774)`.
- The U3V control channel stops returning pending-ack timeout bytes as
  register data — a silent-wrong-data path closes.
- The layering matches the protocols: `viva-gencp` holds what GenCP and
  GVCP actually share, and neither transport is forced to borrow the
  other's vocabulary. The pressure that produced the `0x8006` misreading
  is gone rather than patched.

### Negative

- Breaking change to a published API. `StatusCode` is `pub` in
  `viva-gencp` and appears in `GigeError::Status` and `U3vError::Status`.
  Acceptable at this stage (CLAUDE.md: no backward-compatibility promise
  before 1.0), and it lands with the 0.3.1/0.4.0 window rather than
  mid-cycle.
- Three types instead of one, and a conversion at each transport boundary.
  The alternative — one enum plus a transport discriminant threaded through
  every decode — puts the same branching in a worse place.
- `Unknown(u16)` must stay on all three. Devices send codes no table lists,
  and refusing to represent them would turn a readable error into a parse
  failure.
