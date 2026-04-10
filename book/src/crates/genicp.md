# `viva-gencp` — Transport‑agnostic GenICam Control Primitives

`viva-gencp` provides **transport‑agnostic control messages** and helpers for GenICam device access. It captures the common semantics used by GigE Vision (GVCP control channel), USB3 Vision, and other transports: *read/write registers and memory blocks, status codes, request‑ack correlation, and binary utilities (bitfields, masks).*

> Why a separate crate? It lets higher layers (`viva-genapi`, `viva-genapi-xml`, and the public `viva-genicam` façade) encode control operations **once**, while each transport crate (e.g., `viva-gige`) focuses on socket/I/O details.

---

## Mental model

- **Request → Acknowledge**: Every operation has a request (with a unique `req_id`) and a matching acknowledge with a `status` and optional payload.
- **Operations** (core set):
  - `ReadReg { addr, width }` (8/16/32) and `WriteReg { addr, value }`
  - `ReadMem { addr, len }` and `WriteMem { addr, bytes }`
- **Status mapping**: Acknowledge carries a transport‑neutral `Status` (e.g., `Success`, `AccessDenied`, `BadAddress`, `Timeout`, `Busy`).
- **Segmentation**: Large memory operations may be split across multiple requests based on the transport’s MTU/limit. `viva-gencp` can calculate safe chunk sizes; the transport sends the chunks.

---

## Quick API tour (typical shapes)

> **Note:** Names below mirror the crate’s intent. If your local API differs slightly, the concepts—and the surrounding examples—still apply.

```rust
use viva_gencp::{Request, Reply, Status};

// Build a request
let req = Request::read_reg_u32(0x0010_0200);

// ... send with a transport adapter (e.g., viva-gige) → get raw bytes ...

// Parse reply
let reply = Reply::from_bytes(&buf)?;
match reply.status() {
    Status::Success => {
        let v = reply.as_u32()?;
        println!("value=0x{v:08X}");
    }
    s => anyhow::bail!("device replied with {s:?}"),
}
````

High‑level helpers (bit update):

```rust
use viva_gencp::bitops::{set_bits32, mask32};

// Read‑modify‑write: set bit 7 at address 0x...200
let v = read_u32(adapter, 0x0010_0200).await?;
let v2 = set_bits32(v, mask32(7..=7), true);
write_u32(adapter, 0x0010_0200, v2).await?;
```

---

## Requests & replies

### Requests

A `Request` contains:

* **Operation**: `ReadReg/WriteReg/ReadMem/WriteMem`
* **Address/Length**: 64‑bit addresses supported in the type system; transport may restrict to 32‑bit
* **Width** (for `ReadReg/WriteReg`): 8/16/32
* **ReqId**: incrementing counter used to match the reply

`viva-gencp` ensures **alignment** (e.g., 16‑bit/32‑bit register widths) and **payload sizing** (e.g., memory read length > 0).

### Replies

A `Reply` provides:

* `status(): Status`
* `payload(): &[u8]` (for reads)
* Typed accessors: `as_u8()`, `as_u16()`, `as_u32()`, `as_block()`

### Status codes

`Status` is a transport‑neutral enum, e.g.:

* `Success`
* `NotImplemented` / `InvalidCommand`
* `BadAddress` / `BadValue`
* `AccessDenied` / `Locked`
* `Busy` / `Timeout`
* `ChecksumError` / `ProtocolError`

The transport adapter maps wire‑specific codes (GVCP/U3V/etc.) to this enum.

---

## Endianness, alignment, and masking

* **Wire endianness** is handled inside `viva-gencp`. Public typed accessors return host‑endian values.
* **Alignment**: register widths must match the address alignment (8→any, 16→2‑byte aligned, 32→4‑byte aligned). Helpers assert this early.
* **Bitfields**: `bitops` offers small utilities to extract/update bit ranges without manual shifts/masks.

```rust
use viva_gencp::bitops::{extract_bits32, mask32};
let flags = extract_bits32(0b1011_0001, mask32(4..=7)); // → 0b1011
```

---

## Chunking large memory operations

Transports cap a single message size (e.g., by MTU). Use the provided splitter to iterate safe chunks:

```rust
use viva_gencp::chunk::ChunkPlan;

let plan = ChunkPlan::for_read(/*addr*/ 0x4000_0000, /*len*/ 64 * 1024,
                               /*max_payload*/ 1400, /*align*/ 4)?;
for step in plan {
    // step.addr(), step.len()
    // build Request::read_mem(step.addr(), step.len()) and send
}
```

Recommendations:

* Keep payload under IP fragmentation thresholds (e.g., ≤ 1400 bytes for standard MTU).
* Honour device alignment rules; some devices require 4‑byte granularity for memory access.

---

## Timeouts & retries

`viva-gencp` defines **semantic** timeouts (command vs. memory) as hints. The **transport** enforces socket receive timeouts, retry counts, and backoff. For UDP‑based transports (GigE), retries are essential; for USB3, link‑level reliability reduces the need.

---

## Integrating with transports

Transports implement a minimal trait so `viva-gencp` can *send/receive* bytes:

```rust
pub trait ControlTransport {
    type Error;
    fn next_req_id(&mut self) -> u16;
    async fn send_request(&mut self, req: Request) -> Result<(), Self::Error>;
    async fn recv_reply(&mut self, expect_req_id: u16) -> Result<Reply, Self::Error>;
}
```

`viva-gige` implements this over GVCP sockets; a future `tl-u3v` would use USB3 endpoints. Higher layers (`viva-genapi`) depend only on this trait to perform feature `get/set`.

---

## Working with GenApi (selectors & SwissKnife)

* **Selectors**: When a feature is selector‑dependent, the NodeMap temporarily sets the selector node(s), performs the underlying register op(s) via `viva-gencp`, then restores state as needed.
* **SwissKnife**: Expression nodes *evaluate* by reading inputs (which may be registers or other nodes), computing the result in host code, and returning the computed value to callers.

This means end‑users usually call `get("ExposureTime")`; the NodeMap and `viva-gencp` handle all required register transactions.

---

## Examples

### Read a 32‑bit register

```rust
let v = viva_gencp::helpers::read_u32(&mut adapter, 0x0010_0200).await?;
```

### Write a 32‑bit register

```rust
viva_gencp::helpers::write_u32(&mut adapter, 0x0010_0200, 0x0000_0001).await?;
```

### Read a memory block safely

```rust
let bytes = viva_gencp::helpers::read_block(&mut adapter, 0x4000_0000, 8192).await?;
```

### Modify a bitfield (read‑modify‑write)

```rust
use viva_gencp::bitops::{mask32, set_bits32};
let v = viva_gencp::helpers::read_u32(&mut adapter, REG_CTRL).await?;
let v2 = set_bits32(v, mask32(3..=5), true); // set bits 3..5
viva_gencp::helpers::write_u32(&mut adapter, REG_CTRL, v2).await?;
```

---

## Testing tips

* **Unit tests** for encode/decode, status mapping, and bitops.
* **Golden packets**: store a few known request/ack byte arrays and assert round‑trips.
* **Fuzzing**: run `arbitrary`/`proptest` on `Reply::from_bytes` to harden parsers.
* **Transport mocks**: a simple in‑memory adapter that echoes canned replies makes NodeMap tests deterministic.

---

## Gotchas & best practices

* Always **match `req_id`** in replies; do not accept stale acks.
* Use **bounded chunk sizes** to avoid IP fragmentation on UDP transports.
* Respect **alignment**; some devices NAK misaligned access.
* Keep **timeouts** conservative; some devices do heavy work on first access (e.g., on‑the‑fly XML assembly).
* Prefer **feature‑level APIs** (NodeMap) in apps; use raw `viva-gencp` only for diagnostics and vendor escape hatches.
