# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

genicam-rs is a pure Rust implementation of GenICam ecosystem building blocks with an Ethernet-first focus (GigE Vision). It provides libraries and CLI tools for camera discovery, control, streaming, and feature access.

We do not maintain backward compatibility at this early development stage. The priority is clear design and structure.

## Related Projects

- **genicam-studio** (`../genicam-studio`) — Tauri desktop app that consumes genicam-service via Zenoh. Contains the `genicam_zenoh_api` crate (shared wire types) and a mock camera service.
- **aravis** (`../aravis`) — C library for GenICam cameras. Provides `arv-fake-gv-camera-0.8` used for integration testing.

## Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Integration tests (requires arv-fake-gv-camera-0.8 installed)
cargo test -p genicam --test fake_camera -- --ignored --test-threads=1

# Format check (CI requirement)
cargo fmt --all --check

# Linting (CI runs with warnings-as-errors)
cargo clippy --workspace --all-targets -- -D warnings

# Generate docs
cargo doc --workspace --all-features --no-deps

# Run sensor service
cargo run -p genicam-service -- --iface en0

# Run CLI tool
cargo run -p gencamctl -- list
```

## Architecture

**Layered design (bottom to top):**

```
genicam-service         - Zenoh bridge: real cameras → genicam-studio
    ↓
genicam (facade)        - End-user API: Camera<T>, discovery, streaming
    ↓
genapi-core             - GenApi engine: NodeMap, node evaluation, caching
    ↓
genapi-xml              - XML parsing: GenICam XML → XmlModel IR
    ↓
tl-gige / tl-u3v        - Transport: GVCP/GVSP for GigE, USB3 Vision (planned)
    ↓
genicp                  - Protocol primitives: GenCP encode/decode
```

**Supporting crates:**
- `pfnc` - Pixel Format Naming Convention tables
- `sfnc` - Standard Feature Naming Convention
- `gencamctl` - CLI binary
- `genicam-service` - Zenoh camera service for genicam-studio (depends on `genicam_zenoh_api` from `../genicam-studio`)

## Key Abstractions

**`RegisterIo` trait** (`genapi-core`): Core abstraction for register read/write. Implemented by `GigeDevice` (via async adapter) and `MockIo` for tests.

**`NodeMap`** (`genapi-core`): Parsed from XML, stores nodes by name, tracks dependency graph for cache invalidation. Supports `pValue` delegation (Integer/Float/Enum/Boolean/Command nodes can delegate to IntReg or other backing nodes).

**`Node` enum**: Integer, Float, Enum, Boolean, Command, Category, SwissKnife, Converter, IntConverter, String.

**`GigeDevice`** (`tl-gige`): Async UDP wrapper for GVCP discovery/control and GVSP streaming. Uses proper GVCP wire format (0x42 key byte, 4-byte addresses).

**`DeviceHandle`** (`genicam-service`): Wraps `Camera<GigeRegisterIo>` with `spawn_blocking` for async-safe access from Zenoh queryable handlers.

## Testing

Unit tests are embedded in source modules (`mod tests { }`). Integration tests against `arv-fake-gv-camera-0.8` live in `crates/genicam/tests/fake_camera.rs` (marked `#[ignore]`, require aravis installed).

```bash
# Test single crate
cargo test -p genapi-core

# Integration tests with fake camera (8/11 pass; 3 streaming tests are known-skip on macOS loopback)
cargo test -p genicam --test fake_camera -- --ignored --test-threads=1

# Test with logging
RUST_LOG=debug cargo test --workspace -- --nocapture
```

**Note:** Streaming integration tests (`test_stream_*`, `test_full_lifecycle`) timeout on macOS loopback due to GVSP UDP limitations. They pass on real NICs with physical cameras.

## Documentation

- **mdBook**: `book/` directory - tutorials, architecture, networking cookbook
- **API docs**: Generated via `cargo doc`, published to GitHub Pages
- **Examples**: 16 examples in `crates/genicam/examples/`

## Standards

This codebase implements these EMVA standards:
- **GenApi** - XML-based node description (Tier-1 + Tier-2 including pValue delegation)
- **GVCP/GVSP** - GigE Vision Control/Streaming Protocols
- **GenCP** - Generic Control Protocol
- **PFNC/SFNC** - Pixel Format and Standard Feature Naming Conventions
