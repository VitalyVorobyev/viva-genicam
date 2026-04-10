# genicam-rs Development Roadmap

> **Last Updated:** April 2026
> **Target Hardware:** GigE Vision cameras (tested with aravis fake camera)

## Current State

**Production Readiness Score: 9/10**

All core functionality works end-to-end: discovery, connection with CCP, XML fetch and parse, feature read/write with pValue delegation, GVSP frame streaming, and a Zenoh-based sensor service for genicam-studio integration. 12/12 integration tests pass against `arv-fake-gv-camera` on macOS loopback.

## Completed Phases

### Phase 1: High-Level Streaming API
- `FrameStream` async iterator with auto-resend
- `connect_gige()` / `connect_gige_with_xml()` one-liner connection
- StreamBuilder for stream configuration

### Phase 2: Basler Camera Support
- Converter, IntConverter, String nodes
- SwissKnife: full expression support (arithmetic, comparisons, ternary, logical, bitwise, 20+ math functions)

### Phase 3: GVCP Protocol Compliance (Apr 2026)
- GVCP header format (0x42 key byte + flags byte)
- Discovery payload parsing (correct field offsets)
- ReadMem/WriteMem with 4-byte GVCP addresses
- CCP (Control Channel Privilege) — `claim_control()`/`release_control()`
- Stream channel registers at bootstrap offsets (0x0d00 base)
- macOS support: `Iface::from_system` via `libc::if_nametoindex`
- Loopback discovery (`discover_all()`) for simulated cameras

### Phase 4: GVSP Parser Fix (Apr 2026)
- Correct header layout (format byte at offset 4)
- Leader payload format (reserved + payload_type before timestamp)
- Payload/trailer format code swap (0x02=trailer, 0x03=payload)

### Phase 5: XML Parser Completeness (Apr 2026)
- pValue delegation: Integer, Float, Boolean, Enum, Command nodes
- IntReg, MaskedIntReg parsed as Integer nodes
- IntSwissKnife with hex literal support and Formula tag
- StructReg with StructEntry → Integer nodes with bitfields
- Port node recognition (skipped as transport-level)
- XML entity decoding (`&amp;` → `&`)
- Optional Min/Max (defaults to full range)
- Static `<Value>` constants
- `<pMax>`/`<pMin>` dynamic constraints
- Case-insensitive URL scheme, GenICam standard URL format

### Phase 6: Sensor Service (Apr 2026)
- `viva-service` crate: Zenoh bridge for genicam-studio
- Discovery loop with DeviceAnnounce publishing
- XML, node set/execute/bulk-read queryables
- Acquisition control with FrameStream → FrameHeader → Zenoh publish
- FPS tracking in AcquisitionStatus
- Device-lost detection and cleanup
- Pixel format conversion (pfnc → zenoh_api)
- Initial SFNC node value publishing on connect

### Phase 7: Shared Crate API (SX Handoff, Apr 2026)
- Serde derives on all genapi-xml public types
- Introspection API: `node_names()`, `dependents()`, `categories()`, `kind_name()`, `access_mode()`, `name()`
- `NullIo` for offline XML browsing
- WASM compatibility verified (wasm32-unknown-unknown)
- `fetch_and_load_xml` behind `fetch` feature flag

### Integration Testing
- 12/12 tests pass against `arv-fake-gv-camera` on macOS loopback
- Tests cover: discovery, connection, XML, feature read/write, command execution, frame streaming, dimension validation, full lifecycle

## Remaining Items

### P1: Service Hardening (deferred)
- Watchdog heartbeat (periodic register read)
- Reconnection on transport error with backoff

### P2: Extended GenAPI Attributes
- Visibility (Beginner/Expert/Guru) filtering
- Description, Tooltip, Representation hints

### P3: Documentation
- Complete mdBook tutorials (streaming, troubleshooting)
- FAQ content

### Future
- USB3 Vision transport (skeleton exists in `tl-u3v`)
- GenTL producer (.cti)
- IPv6 support

## Supported Node Types

| Node Type | XML Parsing | Runtime Evaluation | Notes |
|-----------|------------|-------------------|-------|
| Integer | ✅ | ✅ | pValue, pMax/pMin, static Value, bitfields, selectors |
| Float | ✅ | ✅ | pValue, scale/offset |
| Enumeration | ✅ | ✅ | pValue, pValue providers |
| Boolean | ✅ | ✅ | pValue, OnValue/OffValue, bitfields |
| Command | ✅ | ✅ | pValue, CommandValue |
| Category | ✅ | ✅ | |
| SwissKnife | ✅ | ✅ | Full expression + hex literals |
| IntSwissKnife | ✅ | ✅ | Via SwissKnife with Formula tag |
| Converter | ✅ | ✅ | FormulaTo/FormulaFrom |
| IntConverter | ✅ | ✅ | |
| String | ✅ | ✅ | |
| IntReg | ✅ | ✅ | Parsed as Integer |
| MaskedIntReg | ✅ | ✅ | Parsed as Integer |
| StructReg | ✅ | ✅ | StructEntry → Integer with bitfield |
| Port | ✅ (skip) | N/A | Transport-level, not evaluated |
