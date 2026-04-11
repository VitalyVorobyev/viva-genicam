# genicam-rs Development Roadmap

> **Last updated:** April 2026

## What Ships in 0.1.0

The initial release covers the full GigE Vision workflow end-to-end, plus USB3 Vision device control.

### GigE Vision (complete)

- GVCP discovery (broadcast / unicast / loopback)
- GenCP register I/O with retry and backoff
- GVSP streaming with frame reassembly, packet resend, and backpressure
- Automatic packet-size negotiation from MTU
- Multicast stream support (IGMP join/leave)
- Event channel with timestamp mapping
- Action commands with scheduled execution
- Chunk data parsing (timestamp, exposure, gain, line status)
- Extended ID support (64-bit block IDs, 32-bit packet IDs)
- CCP (Control Channel Privilege) claim/release

### GenApi (complete)

- XML parsing into typed intermediate representation
- NodeMap with dependency tracking and cache invalidation
- All standard node types: Integer, Float, Enum, Boolean, Command, Category, String
- SwissKnife / IntSwissKnife with full expression support
- Converter / IntConverter
- IntReg, MaskedIntReg, StructReg with bitfields
- pValue delegation for Integer, Float, Enum, Boolean, Command
- Selector-based address switching
- NullIo for offline XML browsing
- WASM compatible (wasm32-unknown-unknown)

### USB3 Vision (control path complete)

- Bootstrap register parsing (ABRM, SBRM, SIRM)
- GenCP-over-USB register read/write
- Device discovery via rusb
- Low-level bulk-endpoint streaming (`U3vStream`)
- Fake U3V camera for testing

### Service layer (complete for GigE)

- Zenoh bridge for genicam-studio (discovery, XML, node control, acquisition, frame streaming)
- Shared wire types crate (viva-zenoh-api, no Zenoh dependency)

### Testing & tooling

- In-process fake GigE and U3V cameras (no hardware required)
- 175+ tests across the workspace
- CLI tool (viva-camctl) for discovery, feature control, streaming, benchmarking

---

## Planned for 0.2.0

### USB3 Vision integration

The U3V transport layer works, but it's not yet wired into the high-level APIs.

| Item | Description |
|------|-------------|
| `U3vFrameStream` | Async frame iterator wrapping blocking USB bulk reads via `spawn_blocking` |
| `StreamBuilder` for U3V | Configure and start U3V streams through the same API as GigE |
| `viva-service-u3v` real USB | Wire real USB discovery into the service (currently `--fake` only) |
| `viva-camctl stream-usb` | CLI streaming command for USB3 Vision cameras |

### GigE Vision: IP configuration

| Item | Description |
|------|-------------|
| FORCEIP command | GVCP opcode 0x0004 for temporary IP assignment (broadcast, targets device by MAC) |
| Persistent IP registers | Read/write bootstrap registers for persistent IP, subnet, gateway |
| `viva-camctl set-ip` | CLI command for IP configuration |

### Service hardening

| Item | Description |
|------|-------------|
| Heartbeat watchdog | Periodic register read to detect device loss |
| Reconnection | Automatic reconnect with backoff on transport errors |

---

## Future

| Item | Priority | Notes |
|------|----------|-------|
| GenApi visibility filtering | P2 | Beginner / Expert / Guru levels |
| GenApi description & tooltip hints | P2 | Representation, unit, description attributes |
| GenTL producer (.cti) | P3 | C-compatible plugin for third-party GenICam consumers |
| CoaXPress transport | P3 | Requires frame grabber SDK integration |
| IPv6 support | P3 | |

---

## Supported Node Types

| Node Type | XML Parsing | Runtime Evaluation | Notes |
|-----------|:-----------:|:------------------:|-------|
| Integer | yes | yes | pValue, pMax/pMin, static Value, bitfields, selectors |
| Float | yes | yes | pValue, scale/offset |
| Enumeration | yes | yes | pValue |
| Boolean | yes | yes | pValue, OnValue/OffValue, bitfields |
| Command | yes | yes | pValue, CommandValue |
| Category | yes | yes | |
| SwissKnife | yes | yes | Full expression + hex literals |
| IntSwissKnife | yes | yes | Via SwissKnife with Formula tag |
| Converter | yes | yes | FormulaTo / FormulaFrom |
| IntConverter | yes | yes | |
| String | yes | yes | |
| IntReg | yes | yes | Parsed as Integer |
| MaskedIntReg | yes | yes | Parsed as Integer |
| StructReg | yes | yes | StructEntry -> Integer with bitfield |
| Port | yes | N/A | Transport-level, not evaluated |
