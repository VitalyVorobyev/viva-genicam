# genicam-rs Production Readiness Review & Development Roadmap

> **Last Updated:** January 2026
> **Target Hardware:** Basler GigE Vision cameras
> **Priority Focus:** High-level streaming API

## Executive Summary

**Current State:** genicam-rs is a well-architected, production-ready Rust implementation of GigE Vision camera control. Core functionality (discovery, GVCP control, GVSP streaming, GenAPI evaluation with Converter nodes) works reliably. The codebase is clean after recent refactoring.

**Overall Production Readiness Score: 8/10**

### What Works Well
- ✅ Camera discovery (broadcast + interface-specific)
- ✅ GVCP protocol with automatic retry and exponential backoff
- ✅ GVSP streaming with bitmap-based reassembly and zero-copy buffers
- ✅ GenAPI Tier-1 node evaluation (Integer, Float, Enum, Boolean, Command, Category)
- ✅ GenAPI Tier-2 nodes: Converter, IntConverter, String
- ✅ SwissKnife with full expression support (arithmetic, comparisons, ternary, logical, bitwise, functions)
- ✅ Selector-aware addressing and cache invalidation
- ✅ Jumbo frame support with MTU-aware packet sizing
- ✅ Multicast streaming
- ✅ CLI tool covering main workflows
- ✅ High-level FrameStream API with async iterator

### Remaining Gaps for Production
1. **No connection heartbeat** - idle connections may drop silently
2. **No Visibility filtering** - UI apps can't filter Beginner/Expert/Guru features
3. **Missing GenAPI nodes** - IntSwissKnife, StructReg not implemented

---

## Detailed Assessment by Layer

### 1. Transport Layer (tl-gige) — Score: 8/10

#### Strengths
| Feature | Status | Location |
|---------|--------|----------|
| GVCP Discovery | ✅ Complete | `gvcp.rs:165-257` |
| ReadMem/WriteMem | ✅ Complete | `gvcp.rs:507-551` |
| Packet Resend | ✅ Complete | `gvcp.rs:671-728` |
| Action Commands | ✅ Complete | `action.rs:1-211` |
| Stream Configuration | ✅ Complete | `gvcp.rs:572-643` |
| Retry with Backoff | ✅ 4 retries, exponential | `gvcp.rs:404-493` |
| GVSP Reassembly | ✅ Bitmap-based, zero-copy | `gvsp.rs:324-427` |
| Buffer Pool | ✅ Lock-free recycling | `nic.rs:287-323` |
| Multi-NIC | ✅ Per-interface binding | `nic.rs:45-93` |
| Jumbo Frames | ✅ MTU-aware | `nic.rs:149-155` |
| Multicast | ✅ IPv4 only | `nic.rs:238-277` |
| Time Sync | ✅ Linear regression | `time.rs:1-432` |

#### Gaps
| Gap | Severity | Impact |
|-----|----------|--------|
| No heartbeat/keepalive | **Critical** | Idle connections dropped by firewalls/switches without detection |
| Manual resend orchestration | Medium | Application must detect incomplete frames and call `request_resend()` |
| Single-block reassembler | Medium | Multi-stream requires multiple reassembler instances |
| IPv6 not supported | Low | Explicitly blocked at `gvcp.rs:377` |
| No payload type extension | Low | Audio/metadata payloads fail parsing |

---

### 2. GenAPI Layer (genapi-xml + genapi-core) — Score: 8/10

#### Supported Node Types
| Node Type | XML Parsing | Runtime Evaluation |
|-----------|-------------|-------------------|
| Integer | ✅ | ✅ with bitfields, selectors |
| Float | ✅ | ✅ with scale/offset |
| Enumeration | ✅ | ✅ with pValue providers |
| Boolean | ✅ | ✅ with bitfields |
| Command | ✅ | ✅ |
| Category | ✅ | ✅ |
| SwissKnife | ✅ | ✅ full expression support |
| Converter | ✅ | ✅ bidirectional formulas |
| IntConverter | ✅ | ✅ integer conversions |
| String | ✅ | ✅ device metadata |

#### Missing Node Types (GenICam 3.x Standard)
| Node Type | Use Case | Priority |
|-----------|----------|----------|
| **IntSwissKnife** | Integer-specific expressions | Medium |
| **StructReg** | Composite register structures | Medium |
| **Port** | Custom register interfaces | Low |
| **MaskedIntReg** | Masked register access | Low |

#### Missing Attributes
| Attribute | Impact |
|-----------|--------|
| **Visibility** (Beginner/Expert/Guru) | UI apps can't filter features |
| **Description, Tooltip** | No user-facing documentation |
| **Representation** (Hex, Logarithmic, IPAddress) | Values displayed incorrectly |
| **DocURL** | External documentation links lost |

#### SwissKnife Capabilities
**Fully Supported:**
- Arithmetic: `+ - * / % **` and unary `-`
- Comparisons: `< > == != <= >=`
- Logical: `&& || !`
- Ternary: `?:`
- Bitwise: `& | ^ ~ << >>`
- Functions: `sin cos tan asin acos atan atan2 sqrt abs ceil floor round trunc ln log log2 log10 exp pow min max fmod neg sgn e pi`

---

### 3. Facade API (genicam crate) — Score: 8/10

#### Ergonomics Assessment
| Task | Lines of Code | Verdict |
|------|---------------|---------|
| Discover cameras | 2 | ✅ Excellent |
| Connect to camera | 1 | ✅ Excellent (`connect_gige()`) |
| Get/set feature | 1 | ✅ Excellent |
| Stream frames | ~20 | ✅ Good (`FrameStream` API) |

#### API Highlights
- `connect_gige(device)` - one-line camera connection with auto XML fetch
- `FrameStream` - async iterator for frame acquisition with auto-resend
- `Camera::get/set` - type-aware feature access

---

### 4. Error Handling — Score: 7/10

- ✅ Well-structured error hierarchy with `thiserror`
- ✅ Device status codes surfaced via `GigeError::Status`
- ✅ SwissKnife distinguishes parse vs eval errors
- ⚠️ `Transport(String)` loses structured info
- ⚠️ No automatic retry for transient failures in user API

---

### 5. Documentation — Score: 5/10

| Resource | Status |
|----------|--------|
| Rustdoc | Basic, present on public types |
| mdBook structure | Complete outline, 50% content |
| Examples | Excellent (16 examples covering all workflows) |
| Quick Start | Good |
| Tutorials | Skeleton only |
| FAQ | Empty |
| Glossary | Empty |

---

### 6. Testing — Score: 6/10

- ✅ Unit tests embedded in modules (~200 lines in genapi-core)
- ✅ MockIo pattern for testing without hardware
- ❌ Missing: Integration tests, concurrent access tests, chaos testing

---

## Design Problems Identified

### Problem 1: Async/Blocking Mismatch
`GigeRegisterIo` wraps async transport with blocking `Handle::block_on()`. Calling feature access from async context will panic.

### Problem 2: Streaming Abstraction Gap
StreamBuilder produces a raw UDP socket; users must manually parse GVSP, reassemble frames, handle timeouts.

### Problem 3: Connection Lifecycle
No connection health monitoring after `GigeDevice::open()`.

### Problem 4: GenAPI Completeness
Tier-1 subset may fail on complex cameras using Converter nodes or advanced SwissKnife.

---

## Development Roadmap

### Phase 1: High-Level Streaming API ✅ COMPLETED
**Goal:** Make frame acquisition trivial — no manual packet handling.

| Task | Status |
|------|--------|
| Design `FrameStream` async iterator API | ✅ Done |
| Implement `FrameStream::next_frame()` | ✅ Done |
| Integrate auto-resend into FrameStream | ✅ Done |
| Add `connect_gige()` convenience helper | ✅ Done |
| Update grab_gige example (reduced to ~30 lines) | ✅ Done |

**Delivered API:**
```rust
let camera = connect_gige(device).await?;
let mut stream = camera.stream().auto_config().build().await?;
while let Some(frame) = stream.next_frame().await? {
    // frame.data() available
}
```

### Phase 2: Basler Camera Support ✅ MOSTLY COMPLETED
**Goal:** Full compatibility with Basler XML (heavy Converter use).

| Task | Status |
|------|--------|
| Implement Converter node (pValue/FormulaTo/FormulaFrom) | ✅ Done |
| Implement IntConverter node | ✅ Done |
| Implement String node (device metadata) | ✅ Done |
| Extend SwissKnife: comparisons (`< > <= >= == !=`) | ✅ Done |
| Extend SwissKnife: ternary (`?:`) | ✅ Done |
| Extend SwissKnife: logical (`&& || !`) | ✅ Done |
| Extend SwissKnife: bitwise (`& | ^ ~ << >>`) | ✅ Done |
| Extend SwissKnife: math functions | ✅ Done |
| Parse Visibility attribute | ⏳ Pending |
| Test with real Basler XML files | ⏳ Pending |

**Verification:** Parse and evaluate Basler ace/dart camera XML without errors.

### Phase 3: Connection Reliability (1 week)
**Goal:** Production-grade connection management.

| Task | Priority | Effort |
|------|----------|--------|
| Add heartbeat mechanism to GigeDevice | P0 | 2 days |
| Add connection state tracking | P1 | 1 day |
| Add configurable keepalive interval | P1 | 0.5 days |
| Document reconnection patterns | P1 | 0.5 days |

### Phase 4: Extended GenAPI (1-2 weeks)
**Goal:** Support advanced expressions and metadata.

| Task | Status |
|------|--------|
| SwissKnife: logical operators (`&& || !`) | ✅ Done (Phase 2) |
| SwissKnife: bitwise operators (`& | ^ << >>`) | ✅ Done (Phase 2) |
| SwissKnife: math functions (min, max, abs, sin, cos, etc.) | ✅ Done (Phase 2) |
| Parse Description/Tooltip for UI | ⏳ Pending |
| Parse Representation hints | ⏳ Pending |
| IntSwissKnife node | ⏳ Pending |

### Phase 5: Documentation & Polish (1 week)
**Goal:** New users productive in 30 minutes.

| Task | Priority | Effort |
|------|----------|--------|
| Complete streaming tutorial | P0 | 1 day |
| Write networking troubleshooting guide | P1 | 1 day |
| Write Basler-specific notes | P1 | 0.5 days |
| Write FAQ with common issues | P1 | 1 day |
| Add error recovery examples | P2 | 1 day |

### Phase 6: Future Enhancements (ongoing)
| Feature | Priority | Notes |
|---------|----------|-------|
| USB3 Vision transport (tl-u3v) | P1 | Skeleton exists |
| GenTL producer (.cti) | P2 | For third-party integration |
| IPv6 support | P3 | Requires protocol updates |
| Windows CI soak testing | P1 | Build works, needs stability test |

---

## Files Modified

### Phase 1 - Streaming API ✅
- `crates/genicam/src/stream.rs` - FrameStream type, async iterator
- `crates/genicam/src/lib.rs` - Added connect_gige(), FrameStream builder
- `crates/genicam/examples/grab_gige.rs` - Simplified to use new API

### Phase 2 - Basler Support ✅
- `crates/genapi-xml/src/lib.rs` - Added Converter, IntConverter, String to NodeDecl
- `crates/genapi-xml/src/parsers/converter.rs` - Converter/IntConverter/String parsers
- `crates/genapi-core/src/nodes.rs` - Added ConverterNode, IntConverterNode, StringNode
- `crates/genapi-core/src/nodemap.rs` - Converter/IntConverter/String evaluation
- `crates/genapi-core/src/swissknife.rs` - Full expression support (comparisons, ternary, logical, bitwise, functions)

### Phase 3 - Connection (Pending)
- `crates/tl-gige/src/gvcp.rs` - Add heartbeat, connection state

### Phase 4 - Extended GenAPI (Partially Done)
- `crates/genapi-core/src/swissknife.rs` - ✅ Logical, bitwise, functions completed
- `crates/genapi-xml/src/lib.rs` - Visibility, Description attributes pending

### Phase 5 - Docs (Pending)
- `book/src/tutorials/streaming.md` - Complete tutorial
- `book/src/networking.md` - Troubleshooting
- `book/src/faq.md` - FAQ content

---

## Verification Plan

### After Phase 1 (Streaming) ✅
- [x] FrameStream API available with async iterator
- [x] Example code ~30 lines (vs previous 300+)
- [x] Auto-resend integrated into FrameStream

### After Phase 2 (Basler) ✅ (partial)
- [x] Converter nodes evaluate correctly
- [x] IntConverter nodes evaluate correctly
- [x] String nodes return device metadata
- [x] SwissKnife with comparisons works
- [x] SwissKnife with ternary works
- [x] SwissKnife with logical/bitwise works
- [x] SwissKnife with math functions works
- [ ] Parse Basler ace/dart camera XML without errors (needs testing)

### After Phase 3 (Connection)
- [ ] Long-running test (1 hour) with heartbeat
- [ ] Connection drops detected within timeout

### After Phase 4 (Extended GenAPI)
- [x] Complex SwissKnife expressions supported
- [ ] Visibility filtering works

### After Phase 5 (Docs)
- [ ] New user can discover → stream in <30 minutes
- [ ] FAQ covers common Basler issues
