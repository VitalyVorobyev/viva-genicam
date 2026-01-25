# genicam-rs Production Readiness Review & Development Roadmap

> **Last Updated:** January 2026
> **Target Hardware:** Basler GigE Vision cameras
> **Priority Focus:** High-level streaming API

## Executive Summary

**Current State:** genicam-rs is a well-architected, early-production-ready Rust implementation of GigE Vision camera control. Core functionality (discovery, GVCP control, GVSP streaming, GenAPI Tier-1 evaluation) works reliably. The codebase is clean after recent refactoring.

**Overall Production Readiness Score: 7/10**

### What Works Well
- ✅ Camera discovery (broadcast + interface-specific)
- ✅ GVCP protocol with automatic retry and exponential backoff
- ✅ GVSP streaming with bitmap-based reassembly and zero-copy buffers
- ✅ GenAPI Tier-1 node evaluation (Integer, Float, Enum, Boolean, Command, Category)
- ✅ SwissKnife arithmetic expressions
- ✅ Selector-aware addressing and cache invalidation
- ✅ Jumbo frame support with MTU-aware packet sizing
- ✅ Multicast streaming
- ✅ CLI tool covering main workflows

### Critical Gaps for Production
1. **No connection heartbeat** - idle connections may drop silently
2. **Missing GenAPI nodes** - Converter, String, IntSwissKnife not implemented
3. **Manual packet reassembly** - users must write 100+ lines for streaming
4. **Limited SwissKnife** - no comparisons, bitwise ops, or math functions
5. **No Visibility filtering** - UI apps can't filter Beginner/Expert/Guru features

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

### 2. GenAPI Layer (genapi-xml + genapi-core) — Score: 6/10

#### Supported Node Types
| Node Type | XML Parsing | Runtime Evaluation |
|-----------|-------------|-------------------|
| Integer | ✅ | ✅ with bitfields, selectors |
| Float | ✅ | ✅ with scale/offset |
| Enumeration | ✅ | ✅ with pValue providers |
| Boolean | ✅ | ✅ with bitfields |
| Command | ✅ | ✅ |
| Category | ✅ | ✅ |
| SwissKnife | ✅ | ✅ arithmetic only |

#### Missing Node Types (GenICam 3.x Standard)
| Node Type | Use Case | Priority |
|-----------|----------|----------|
| **Converter/IntConverter** | Type conversions, non-linear mappings | **High** |
| **String** | Device name, serial number, model | **High** |
| **IntSwissKnife** | Integer-specific expressions with bitwise ops | Medium |
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

#### SwissKnife Limitations
**Supported:** `+ - * / ( )` and unary `-`

**NOT Supported:**
- Comparisons: `< > == != <= >=`
- Logical: `&& || !`
- Ternary: `?:`
- Bitwise: `& | ^ ~ << >>`
- Functions: `sin cos sqrt abs min max pow log`

---

### 3. Facade API (genicam crate) — Score: 7/10

#### Ergonomics Assessment
| Task | Lines of Code | Verdict |
|------|---------------|---------|
| Discover cameras | 2 | ✅ Excellent |
| Connect to camera | 8-12 | ⚠️ Boilerplate-heavy |
| Get/set feature | 1 | ✅ Excellent |
| Stream frames | 100+ | ❌ Too low-level |

#### API Friction Points
1. **XML fetching boilerplate** (should be 1 line, currently 6)
2. **Streaming requires manual packet reassembly** (300+ lines in examples)
3. **String-based feature access lacks type safety**

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

### Phase 1: High-Level Streaming API (2 weeks)
**Goal:** Make frame acquisition trivial — no manual packet handling.

| Task | Priority | Effort |
|------|----------|--------|
| Design `FrameStream` async iterator API | P0 | 1 day |
| Implement `FrameStream::next_frame()` | P0 | 3 days |
| Integrate auto-resend into FrameStream | P0 | 2 days |
| Add `Camera::connect()` convenience helper | P0 | 1 day |
| Add frame callback variant | P1 | 1 day |
| Update grab_gige example (reduce to ~20 lines) | P1 | 1 day |
| Add streaming integration test | P1 | 1 day |

**Deliverable:** User can stream frames with:
```rust
let camera = Camera::connect(device).await?;
let mut stream = camera.stream().auto_config().build().await?;
while let Some(frame) = stream.next_frame().await? {
    // frame.to_rgb8() available
}
```

### Phase 2: Basler Camera Support (2 weeks)
**Goal:** Full compatibility with Basler XML (heavy Converter use).

| Task | Priority | Effort |
|------|----------|--------|
| Implement Converter node (pFrom/pTo/Formula) | P0 | 3 days |
| Implement IntConverter node | P0 | 2 days |
| Implement String node (device metadata) | P0 | 1 day |
| Extend SwissKnife: comparisons (`< > == !=`) | P0 | 2 days |
| Extend SwissKnife: ternary (`?:`) | P1 | 1 day |
| Parse Visibility attribute | P1 | 1 day |
| Test with real Basler XML files | P1 | 2 days |

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

| Task | Priority | Effort |
|------|----------|--------|
| SwissKnife: logical operators (`&& || !`) | P1 | 1 day |
| SwissKnife: bitwise operators (`& | ^ << >>`) | P1 | 2 days |
| SwissKnife: math functions (min, max, abs) | P2 | 2 days |
| Parse Description/Tooltip for UI | P2 | 1 day |
| Parse Representation hints | P2 | 2 days |
| IntSwissKnife node | P2 | 2 days |

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

## Files to Modify

### Phase 1 - Streaming API
- `crates/genicam/src/stream.rs` (NEW) - FrameStream type, async iterator
- `crates/genicam/src/lib.rs` - Add Camera::connect(), stream() builder
- `crates/tl-gige/src/gvsp.rs` - Auto-resend integration
- `crates/genicam/examples/grab_gige.rs` - Simplify to use new API

### Phase 2 - Basler Support
- `crates/genapi-xml/src/lib.rs` - Add Converter, String to NodeDecl
- `crates/genapi-xml/src/parsers/converter.rs` (NEW) - Converter parser
- `crates/genapi-xml/src/parsers/string.rs` (NEW) - String parser
- `crates/genapi-core/src/nodes.rs` - Add ConverterNode, StringNode
- `crates/genapi-core/src/nodemap.rs` - Converter/String evaluation
- `crates/genapi-core/src/swissknife.rs` - Comparisons, ternary

### Phase 3 - Connection
- `crates/tl-gige/src/gvcp.rs` - Add heartbeat, connection state

### Phase 4 - Extended GenAPI
- `crates/genapi-core/src/swissknife.rs` - Logical, bitwise, functions
- `crates/genapi-xml/src/lib.rs` - Visibility, Description attributes

### Phase 5 - Docs
- `book/src/tutorials/streaming.md` - Complete tutorial
- `book/src/networking.md` - Troubleshooting
- `book/src/faq.md` - FAQ content

---

## Verification Plan

### After Phase 1 (Streaming)
- [ ] Stream 1000 frames with new FrameStream API
- [ ] Example code < 30 lines (vs current 300+)
- [ ] Auto-resend works on induced packet loss
- [ ] Sustain 900 Mb/s on 1 GbE with <0.1% drops

### After Phase 2 (Basler)
- [ ] Parse Basler ace/dart camera XML without errors
- [ ] Converter nodes evaluate correctly
- [ ] String nodes return device metadata
- [ ] SwissKnife with comparisons works

### After Phase 3 (Connection)
- [ ] Long-running test (1 hour) with heartbeat
- [ ] Connection drops detected within timeout

### After Phase 4 (Extended GenAPI)
- [ ] Complex SwissKnife expressions from Basler XML
- [ ] Visibility filtering works

### After Phase 5 (Docs)
- [ ] New user can discover → stream in <30 minutes
- [ ] FAQ covers common Basler issues
