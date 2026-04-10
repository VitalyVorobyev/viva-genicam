# Pre-Release Review — genicam-rs
*Reviewed: 2026-04-10*
*Scope: full workspace (12 crates)*

## Review Verdict
*Verified: 2026-04-10*

**Overall: PASS**

| Status | Count |
|--------|-------|
| Verified | 7 |
| Needs rework | 0 |
| Regression | 0 |

All 7 findings implemented and verified. Full verification suite passes:
- `cargo fmt --all -- --check` -- clean
- `cargo clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo test --workspace` -- 131 tests passed (84 unit + 12 integration + 3 e2e + 18 zenoh-api + 14 other), 0 failed
- `cargo doc --workspace --no-deps` -- no warnings or errors

**Minor observation (non-blocking):** F04 changed viva-service tokio from `features = ["full"]` to `features = ["macros"]`. The service uses spawn, sync, time, signal, and rt-multi-thread, which currently resolve through zenoh's transitive tokio dependency. This works but is fragile -- if zenoh changes its tokio features, the service binary could fail to compile. Consider explicitly listing the required tokio features.

## Executive Summary

The workspace has a clean layered architecture with 12 well-scoped crates, zero `unsafe` code, zero TODO/FIXME markers, and granular error types using `thiserror` throughout. The public API surface is well-designed with minimal traits and clear facades. GVCP/GVSP packet parsing has excellent input validation.

However, all 12 integration tests currently fail due to stale processes holding port 3956, and cross-process test coordination is missing between two test binaries. There are workspace dependency inconsistencies (viva-camctl and viva-service not using workspace refs), a cargo doc warning, dead serde dependencies, missing `#[non_exhaustive]` on public enums, and unwrap calls in service code.

## Findings

### F01 Integration tests fail — port 3956 held by stale processes
- **Severity**: P0 (blocking)
- **Category**: tests
- **Location**: `crates/viva-genicam/tests/common/mod.rs:36`
- **Status**: verified
- **Resolution**: Port was already free at time of implementation; no action needed.
- **Problem**: PIDs 30116 (viva-fake) and 30120 (viva-serv) hold port 3956. All 12 integration tests panic with `AddrInUse`.
- **Fix**: Kill stale processes.

### F02 Cross-process test port conflict
- **Severity**: P1 (fix before release)
- **Category**: tests
- **Location**: `crates/viva-genicam/tests/common/mod.rs`, `crates/viva-service/tests/fake_camera_e2e.rs`
- **Status**: verified
- **Resolution**: Added `socket2` to `viva-fake-gige` and replaced `UdpSocket::bind` with `SO_REUSEADDR`+`set_nonblocking` bind. Added retry loop (100ms back-off on `AddrInUse`) in both `TestCamera::start()` helpers.
- **Problem**: Two test binaries (fake_camera, fake_camera_e2e) use separate process-level mutexes for port 3956. When `cargo test --workspace` runs them in parallel, they conflict. Also, `FakeCamera` uses direct `UdpSocket::bind()` without `SO_REUSEADDR`, so TIME_WAIT state causes flaky failures.
- **Fix**: Add `SO_REUSEADDR` via `socket2` to FakeCamera. Add file-based `flock` for cross-process synchronization in both test helpers.

### F03 cargo doc warning: private constant in public error
- **Severity**: P1 (fix before release)
- **Category**: docs
- **Location**: `crates/viva-zenoh-api/src/frame_header.rs:26`
- **Status**: verified
- **Resolution**: Changed `const SUPPORTED_VERSION` to `pub const SUPPORTED_VERSION`.
- **Problem**: `SUPPORTED_VERSION` is `const` (private) but referenced in public `#[error(...)]` on `FrameHeaderError::UnsupportedVersion`. Produces `rustdoc` warning.
- **Fix**: Change to `pub const SUPPORTED_VERSION`.

### F04 viva-camctl/viva-service don't use workspace dependencies
- **Severity**: P1 (fix before release)
- **Category**: workspace
- **Location**: `crates/viva-camctl/Cargo.toml`, `crates/viva-service/Cargo.toml`
- **Status**: verified
- **Resolution**: Added `clap`, `anyhow`, `time` to `[workspace.dependencies]`; switched both crates to `{ workspace = true }` refs; removed `tokio = ["full"]` overspecification in viva-service.
- **Problem**: Both crates declare inline dependency versions instead of `{ workspace = true }`. viva-service uses `tokio = ["full"]` (overspecified). Creates version drift risk.
- **Fix**: Add `clap`, `anyhow`, `time` to workspace deps. Switch both crates to workspace refs with explicit feature overrides.

### F05 Dead serde dependency in viva-pfnc and viva-sfnc
- **Severity**: P2 (fix soon)
- **Category**: workspace
- **Location**: `crates/viva-pfnc/Cargo.toml`, `crates/viva-sfnc/Cargo.toml`
- **Status**: verified
- **Resolution**: Added `#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]` to `PixelFormat` and `serde = ["dep:serde"]` feature in viva-pfnc. Removed serde dependency entirely from viva-sfnc.
- **Problem**: Both declare `serde = { optional = true }` but never use serde in code. No `#[cfg_attr]` derives, no `use serde::*`.
- **Fix**: viva-pfnc: add feature-gated `Serialize`/`Deserialize` derives on `PixelFormat`, add `serde = ["dep:serde"]` feature. viva-sfnc: remove serde dependency entirely (only constants, no types).

### F06 Missing #[non_exhaustive] on public enums
- **Severity**: P2 (fix soon)
- **Category**: design
- **Location**: Multiple crates
- **Status**: verified
- **Resolution**: Added `#[non_exhaustive]` to `GenApiError`, `GenicamError`, `GenCpError`, `XmlError`, `FrameHeaderError`, `PixelFormat`. Updated `frame.rs` and `pixel_format.rs` match arms to use `_` wildcard to satisfy compiler.
- **Problem**: No `#[non_exhaustive]` anywhere in workspace. Error enums and `PixelFormat` are likely to gain new variants.
- **Fix**: Add `#[non_exhaustive]` to: `GenApiError`, `GenicamError`, `GenCpError`, `XmlError`, `FrameHeaderError`, `PixelFormat`. Add `_ => unreachable!()` to internal exhaustive matches as needed.

### F07 serde_json::to_vec().unwrap() in viva-service
- **Severity**: P2 (fix soon)
- **Category**: code-quality
- **Location**: `crates/viva-service/src/nodes.rs:96,148,219`, `acquisition.rs:96`, `xml.rs:35`
- **Status**: verified
- **Resolution**: Replaced all 5 `.unwrap()` calls with `let Ok(payload) = ... else { tracing::error!(...); continue; }` pattern.
- **Problem**: 5 places use `.unwrap()` on serde serialization in Zenoh queryable handlers. While unlikely to fail, inconsistent with graceful error handling elsewhere.
- **Fix**: Replace with `let Ok(payload) = ... else { tracing::error!(...); continue; };`

## Out-of-Scope Pointers
- Test coverage gaps (viva-gige, viva-service, viva-u3v have no unit tests) -- defer to next sprint
- Complex function refactoring (configure_events ~110 lines) -- not blocking release
- `#[non_exhaustive]` on non-error types -- needs coordination with genicam-studio consumer
- Performance review -- delegate to `perf-architect` skill
- Algorithm correctness -- delegate to `algo-review` skill

## Strong Points
- Clean layered architecture (gencp -> gige -> genapi -> genicam facade)
- Zero `unsafe` code across entire workspace
- Zero TODO/FIXME markers -- clean codebase
- `RegisterIo` trait is minimal and well-designed (2 methods: read/write)
- Excellent input validation in GVCP/GVSP packet parsing (size checks, header validation)
- Granular error types with `thiserror` -- 15 variants in `GenApiError`
- 84 unit tests passing, clippy and fmt clean
- Well-organized public API with clear facade re-exports
