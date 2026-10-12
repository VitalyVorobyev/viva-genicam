# `viva-gencp` — GenCP message primitives

`viva-gencp` is the smallest crate in the stack: one `lib.rs` with **GenCP's
vocabulary** — opcodes, command ids, flags and status codes — and the decoding
of an acknowledgement, and nothing else. No sockets, no retries, no async, no
device state.

GigE Vision and USB3 Vision both control a camera with GenCP-style register
reads and writes, but they do not put the same bytes on the wire:

| | GigE Vision (GVCP) | USB3 Vision (GenCP proper) |
|---|---|---|
| Read / write memory | opcodes `0x0084` / `0x0086` | command ids `0x0800` / `0x0802` |
| Register commands | `0x0080` / `0x0082` | none: a register is a 4-byte memory access |
| "Answer me" flag | bit 0 of the flag byte | bit 14 of the 16-bit flags |
| Byte order | big-endian | little-endian |

What they share is a core of status codes and the pending-acknowledge
mechanism. This crate holds both tables: `OpCode` is GVCP's, and
`command_id` and `ccd_flags` are GenCP's (GenCP 1.2, Tables 4 and 7). Until
0.8 the USB3 Vision transport sent GVCP's opcodes, which no USB3 Vision camera
understands.

---

## What is actually in it

| Item | Purpose |
|---|---|
| `OpCode` | GVCP's `ReadRegister`, `WriteRegister`, `ReadMem`, `WriteMem` |
| `command_id` | GenCP's command ids: `READMEM_CMD` `0x0800` … `EVENT_ACK` `0x0C01` |
| `ccd_flags` | GenCP's `REQUEST_ACK` (bit 14) and `COMMAND_RESEND` (bit 15) |
| `StatusCode` | The status codes GVCP and GenCP share; each transport wraps it in its own type |
| `AckHeader` | GVCP's 8-byte acknowledgement header, its status as the wire value |
| `GenCpAck` | Header plus payload |
| `decode_ack` | Parses a GVCP acknowledgement; `viva-gige` uses it |
| `GenCpError` | Decode failures |
| `HEADER_SIZE`, `PENDING_ACK_COMMAND` | `8` and `0x0805` |

`OpCode::command_code()` gives GVCP's wire value (`0x0080`, `0x0082`, `0x0084`,
`0x0086`); `StatusCode::from_raw` / `to_raw` convert a shared status code.

---

## Request → acknowledge

Every command carries a **request id**, and the acknowledgement echoes it. The
transport is responsible for matching them and for discarding stale acks — a
late reply to a timed-out request must not be accepted as the answer to the
next one.

Callers do not usually touch this crate. `GigeDevice` and `U3vDevice` build the
commands, and `NodeMap` sits above those, so an application reads
`ExposureTime` and the register transactions happen underneath.

---

## Status codes

The table matters more than it looks, because a status is often the only thing
a user sees when something fails. `StatusCode` holds the codes both protocols
define, with the same meaning: `0x0000`, `0x8001`–`0x8007`, `0x800E`, `0x800F`
and `0x8FFF`.

| Code | Meaning | Retry? |
|---|---|---|
| `Success` | Completed | — |
| `NotImplemented` | Device does not implement this command | No |
| `InvalidParameter` | Parameter invalid or out of range | No |
| `InvalidAddress` | No such address on the device | No |
| `WriteProtect` | Register is read-only | No — permanent |
| `BadAlignment` | Access not aligned as the transport requires | No |
| `AccessDenied` | Refused *this* write: a GenApi lock, or control privilege not held | Only after fixing the cause |
| `Busy` | Device busy | **Yes** — the only one worth retrying |
| `InvalidHeader` | The command's header is invalid | No |
| `WrongConfig` | The device's configuration does not allow the command | No |
| `GenericError` | Device reported an error with nothing more specific | — |

Beyond that the two tables part, and at `0x800B` they disagree: GVCP's
`NO_MSG`, GenCP's `MSG_TIMEOUT`. So each transport decodes a status with a type
of its own that wraps this one — `viva_gige::GigeStatus` (with GVCP's
packet-resend codes) and `viva_u3v::U3vStatus` (with USB3 Vision's `0xA0xx`
codes) — and keeps `Unknown(u16)` for a code neither table names. Each prints
the name and the hex value together, `ACCESS_DENIED (0x8006)`.

The distinction between `WriteProtect` and `AccessDenied` is worth keeping in
mind when debugging: the first means the register never accepts writes, the
second means it would but not right now.

### Pending acknowledge

GenCP lets a device say "still working" rather than answering immediately. It
signals that with a **command id** — `PENDING_ACK_COMMAND`, `0x0805` — not with
a status code, so a transport has to check the acknowledge's command id before
it interprets the status. Confusing the two turns an access-denied answer into
a pending acknowledge, or the other way round.

---

## Using it directly

You would only do this for diagnostics. `decode_ack` turns the bytes of a GVCP
acknowledgement into a `GenCpAck`:

```rust,ignore
use viva_gige::GigeStatus;
use viva_gencp::{StatusCode, decode_ack};

let ack = decode_ack(&buf)?;        // -> GenCpAck

// `AckHeader::status` is the wire value: decode it with the transport's type,
// which names GVCP's own codes as well as the shared ones.
match GigeStatus::from_raw(ack.header.status) {
    GigeStatus::Common(StatusCode::Success) => { /* ack.payload */ }
    GigeStatus::Common(StatusCode::Busy) => { /* the one status worth retrying */ }
    other => return Err(format!("device reported {other}").into()),
}
```

`ack.header.request_id` is what the transport matches against the request it
sent, and `OpCode::ack_code()` (`command_code() + 1`) is the opcode it should
have come back with.

To send commands, use the layers that build them correctly:
`viva_gige::GigeDevice::{read_register, write_register, read_mem, write_mem}`,
or `Camera::{get, set}` above them.

---

## Endianness and alignment

GenCP leaves the byte order to the transport, but requires it to match the
device's bootstrap registers (GenCP 1.2 §2.3): GVCP is big-endian, USB3
Vision little-endian. `decode_ack` is GVCP's and handles the big-endian side;
`viva-u3v` encodes its own little-endian packets. The structs
hold host-order values. Register widths must match the address
alignment, and devices do return `BadAlignment` when they do not — which is why
that status has a name of its own rather than being folded into
`InvalidParameter`.

---

## See also

- [`viva-gige`](viva-gige.md) — GenCP over GVCP, plus GVSP streaming
- [`viva-genapi`](viva-genapi.md) — the NodeMap that turns feature names into
  these messages
