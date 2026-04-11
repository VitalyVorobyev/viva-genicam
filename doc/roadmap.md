# genicam-rs Development Roadmap

> **Last updated:** April 11, 2026

## What Ships in 0.1.0

The initial release covers the full GigE Vision workflow end-to-end, plus USB3 Vision device control and streaming.

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
- FORCEIP command for temporary IP assignment
- Persistent IP configuration (read/write/enable)
- CLI IP configuration (`viva-camctl set-ip`)

### GenApi (complete)

- XML parsing into typed intermediate representation
- NodeMap with dependency tracking and cache invalidation
- All standard node types: Integer, Float, Enum, Boolean, Command, Category, String
- SwissKnife / IntSwissKnife with full expression support
- Converter / IntConverter
- IntReg, MaskedIntReg, StructReg with bitfields
- pValue delegation for Integer, Float, Enum, Boolean, Command
- Selector-based address switching
- Node metadata: Visibility, Description, ToolTip, DisplayName, Representation
- Visibility filtering (`nodes_at_visibility`)
- NullIo for offline XML browsing
- WASM compatible (wasm32-unknown-unknown)

### USB3 Vision (complete)

- Bootstrap register parsing (ABRM, SBRM, SIRM)
- GenCP-over-USB register read/write
- Device discovery via rusb
- Low-level bulk-endpoint streaming (`U3vStream`)
- Async frame iterator (`U3vFrameStream`) wrapping blocking bulk reads
- Stream builder (`U3vStreamBuilder`) for configuring U3V streams
- Real USB camera support in `viva-service-u3v`
- CLI streaming command (`viva-camctl stream-usb`)
- Fake U3V camera for testing

### Service layer (complete)

- Zenoh bridge for genicam-studio (discovery, XML, node control, acquisition, frame streaming)
- Shared wire types crate (viva-zenoh-api, no Zenoh dependency)
- Reconnection with exponential backoff (GigE)

### Testing & tooling

- In-process fake GigE and U3V cameras (no hardware required)
- 175+ tests across the workspace
- CLI tool (viva-camctl) for discovery, feature control, streaming, benchmarking

---

## Shipped in 0.2.0

### USB3 Vision integration

| Item | Status |
|------|--------|
| `U3vFrameStream` | done |
| `U3vStreamBuilder` | done |
| `viva-service-u3v` real USB | done |
| `viva-camctl stream-usb` | done |

### GigE Vision: IP configuration

| Item | Status |
|------|--------|
| FORCEIP command (opcode 0x0004) | done |
| Persistent IP registers | done |
| `viva-camctl set-ip` | done |

### Service hardening

| Item | Status |
|------|--------|
| Heartbeat watchdog | done (0.1.0) |
| Reconnection with backoff | done |

---

## Shipped in 0.2.1

### GenApi metadata

| Item | Status |
|------|--------|
| Visibility filtering (Beginner / Expert / Guru / Invisible) | done |
| Description & tooltip parsing | done |
| DisplayName parsing | done |
| Representation hints (Linear, Logarithmic, HexNumber, etc.) | done |
| `NodeMap::nodes_at_visibility(level)` filtering | done |

---

## Future

| Item | Notes |
|------|-------|
| GenTL producer (.cti) | C-compatible plugin for third-party GenICam consumers |
| CoaXPress transport | Requires frame grabber SDK integration |
| IPv6 support | |

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
