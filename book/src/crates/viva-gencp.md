# `viva-gencp` — GenCP message primitives

`viva-gencp` is the smallest crate in the stack: one `lib.rs` that encodes and
decodes **GenCP control messages** and nothing else. No sockets, no retries, no
async, no device state.

That is the point. GigE Vision carries GenCP over UDP (GVCP) and USB3 Vision
carries it over bulk endpoints; the message layout, the opcodes and the status
codes are the same in both. Keeping them in a transport-free crate means the
two transports share one definition rather than two that drift.

---

## What is actually in it

| Item | Purpose |
|---|---|
| `OpCode` | `ReadRegister`, `WriteRegister`, `ReadMem`, `WriteMem` |
| `StatusCode` | Transport-neutral status, with `Unknown(u16)` for anything not in the table |
| `CommandHeader` / `AckHeader` | The 8-byte headers |
| `GenCpCmd` / `GenCpAck` | Header plus payload |
| `encode_cmd` / `decode_ack` | The two functions you call |
| `GenCpError` | Decode failures |
| `HEADER_SIZE`, `PENDING_ACK_COMMAND` | `8` and `0x0805` |

`OpCode::command_code()` gives the wire value (`0x0080`, `0x0082`, `0x0084`,
`0x0086`); `StatusCode::from_raw` / `to_raw` convert the status field.

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
a user sees when something fails.

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
| `GenericError` | Device reported an error with nothing more specific | — |
| `Unknown(u16)` | Not in this table, or transport-specific; carries the raw value |  |

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

You would only do this for diagnostics or a vendor escape hatch. The shape:

```rust,ignore
use viva_gencp::{StatusCode, decode_ack, encode_cmd};

let bytes = encode_cmd(&cmd);       // -> Bytes, ready for the transport
// ... the transport sends `bytes` and receives `buf` ...
let ack = decode_ack(&buf)?;        // -> GenCpAck

// `AckHeader::status` is already a decoded `StatusCode`; `from_raw`/`to_raw`
// are there for transports that need the wire value.
match ack.header.status {
    StatusCode::Success => { /* ack.payload */ }
    StatusCode::Busy => { /* the one status worth retrying */ }
    other => return Err(format!("device reported {other}").into()),
}
```

`ack.header.request_id` is what the transport matches against the request it
sent, and `OpCode::ack_code()` (`command_code() + 1`) is the opcode it should
have come back with.

For anything above raw diagnostics, prefer the layers that already do this
correctly: `viva_gige::GigeDevice::{read_register, write_register, read_mem,
write_mem}`, or `Camera::{get, set}` above them.

---

## Endianness and alignment

GenCP is big-endian on the wire; `encode_cmd` and `decode_ack` handle that, and
the structs hold host-order values. Register widths must match the address
alignment, and devices do return `BadAlignment` when they do not — which is why
that status has a name of its own rather than being folded into
`InvalidParameter`.

---

## Testing

Encode/decode is exactly the kind of code that should be tested against the
**specification**, not against the parser. Fixtures derived from the parser
only assert that the code agrees with itself: when a fake camera and the client
share one wrong assumption, the test asserts it back. See
[ADR-0018](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0018-genapi-conformance-over-convenience.md).

---

## See also

- [`viva-gige`](viva-gige.md) — GenCP over GVCP, plus GVSP streaming
- [`viva-genapi`](viva-genapi.md) — the NodeMap that turns feature names into
  these messages
