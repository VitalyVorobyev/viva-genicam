# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

genicam-rs is a pure Rust implementation of GenICam ecosystem building blocks supporting GigE Vision and USB3 Vision. It provides libraries and CLI tools for camera discovery, control, streaming, and feature access.

We do not maintain backward compatibility at this early development stage. The priority is clear design and structure.

## Related Projects

- **genicam-studio** (`../genicam-studio`) — Tauri desktop app that consumes viva-service via Zenoh. Contains the `viva_zenoh_api` crate (shared wire types) and a mock camera service.
- **aravis** (`../aravis`) — C library for GenICam cameras. Optional; used only for conformance testing against a third-party implementation. Not required for development or CI.

## Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Integration tests (uses in-process fake camera, no external tools needed)
cargo test -p viva-genicam --test fake_camera

# Format check (CI requirement)
cargo fmt --all --check

# Linting (CI runs with warnings-as-errors)
cargo clippy --workspace --all-targets -- -D warnings

# Generate docs
cargo doc --workspace --all-features --no-deps

# Run sensor service
cargo run -p viva-service -- --iface en0

# Run CLI tool
cargo run -p viva-camctl -- list
```

## Architecture

**Layered design (bottom to top):**

```
viva-service            - Zenoh bridge: GigE cameras → genicam-studio
viva-service-u3v        - Zenoh bridge: U3V cameras → genicam-studio
    ↓
viva-genicam (facade)   - End-user API: Camera<T>, discovery, streaming
    ↓
viva-genapi             - GenApi engine: NodeMap, node evaluation, caching
    ↓
viva-genapi-xml         - XML parsing: GenICam XML → XmlModel IR
    ↓
viva-gige / viva-u3v    - Transport: GVCP/GVSP for GigE, USB3 Vision
    ↓
viva-gencp              - Protocol primitives: GenCP encode/decode
```

**Supporting crates:**
- `viva-pfnc` - Pixel Format Naming Convention tables
- `viva-sfnc` - Standard Feature Naming Convention
- `viva-zenoh-api` - Shared Zenoh wire types (no Zenoh dependency)
- `viva-camctl` - CLI binary (not published)
- `viva-service` - GigE Zenoh camera service for genicam-studio
- `viva-service-u3v` - U3V Zenoh camera service for genicam-studio (supports `--fake` mode and real USB)
- `viva-fake-gige` - In-process fake GigE Vision camera for testing (not published)
- `viva-fake-u3v` - In-process fake USB3 Vision camera for testing (not published)

## Key Abstractions

**`RegisterIo` trait** (`viva-genapi`): Core abstraction for register read/write. Implemented by `GigeRegisterIo` (async-to-sync adapter using `block_in_place` + `block_on`, safe from both async and sync contexts), `MockIo` for tests, and `NullIo` for offline browsing.

**`NodeMap`** (`viva-genapi`): Parsed from XML, stores nodes by name, tracks dependency graph for cache invalidation. Supports `pValue` delegation (Integer/Float/Enum/Boolean/Command nodes can delegate to IntReg or other backing nodes).

**`Node` enum**: Integer, Float, Enum, Boolean, Command, Category, SwissKnife, Converter, IntConverter, String.

**`GigeDevice`** (`viva-gige`): Async UDP wrapper for GVCP discovery/control and GVSP streaming. Uses proper GVCP wire format (0x42 key byte, 4-byte addresses).

**`U3vFrameStream`** (`viva-genicam`): Async frame iterator wrapping blocking USB bulk reads via `spawn_blocking` + mpsc channel. Created via `U3vStreamBuilder` or `U3vFrameStream::start()`.

**`U3vDeviceHandle<T>`** (`viva-service-u3v`): Generic over `T: UsbTransfer`, works with both `FakeU3vTransport` and `RusbTransfer`.

**`DeviceHandle`** (`viva-service`): Wraps `Camera<GigeRegisterIo>` with `spawn_blocking` for async-safe access from Zenoh queryable handlers. Includes reconnection with exponential backoff.

## Testing

Unit tests are embedded in source modules (`mod tests { }`). Integration tests use `viva-fake-gige` (in-process fake camera) and run automatically -- no external tools or hardware required.

```bash
# All tests (unit + integration + service e2e)
cargo test --workspace

# GigE integration tests (12 tests: discovery, features, streaming)
cargo test -p viva-genicam --test fake_camera

# U3V integration tests (5 tests: open, features, streaming, pixel formats)
cargo test -p viva-genicam --test fake_u3v_camera

# Service end-to-end tests (3 tests: acquisition, double-start, sustained streaming)
cargo test -p viva-service --test fake_camera_e2e

# Test with logging
RUST_LOG=debug cargo test --workspace -- --nocapture
```

### Fake camera binary

For interactive testing or E2E testing with genicam-studio:

```bash
# Start fake camera (stays alive until Ctrl+C)
cargo run -p viva-fake-gige
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15 --pixel-format rgb8

# Use CLI to interact
cargo run -p viva-camctl -- list --iface 127.0.0.1

# E2E with studio — GigE (3 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-service -- --iface lo0 --zenoh-config ../genicam-studio/config/zenoh-local.json5
# T3: cd ../genicam-studio/apps/genicam-studio-tauri && cargo tauri dev

# E2E with studio — USB3 Vision fake camera (2 terminals)
# T1: cargo run -p viva-service-u3v -- --fake --zenoh-config ../genicam-studio/config/zenoh-local.json5
# T2: cd ../genicam-studio/apps/genicam-studio-tauri && cargo tauri dev
```

```bash
# Test FORCEIP with fake GigE camera (2 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --force --iface 127.0.0.1

# Test persistent IP with fake GigE camera (2 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --iface 127.0.0.1
```

**Important**: The `--zenoh-config` flag pointing to `zenoh-local.json5` is required on the **service** side (both GigE and U3V) when connecting to genicam-studio. The studio loads its own Zenoh config automatically in dev mode (`cargo tauri dev`).

## Documentation

- **mdBook**: `book/` directory - tutorials, architecture, networking cookbook
- **API docs**: Generated via `cargo doc`, published to GitHub Pages
- **Examples**: 17 examples in `crates/viva-genicam/examples/` (including `demo_fake_camera` for zero-hardware demo)

## Shared Crate API (SX handoff)

`viva-genapi-xml` and `viva-genapi` are designed for external consumption by genicam-studio:
- All `viva-genapi-xml` public types derive `Serialize`/`Deserialize` (serde)
- `viva-genapi` provides introspection: `NodeMap::node_names()`, `dependents()`, `categories()`, `Node::kind_name()`, `access_mode()`, `name()`
- `NullIo` enables offline XML browsing without a camera
- Both crates compile for `wasm32-unknown-unknown`
- `fetch_and_load_xml` is behind the `fetch` feature flag (default on)

## Standards

This codebase implements these EMVA standards:
- **GenApi** - XML-based node description (Tier-1 + Tier-2 including pValue delegation)
- **GVCP/GVSP** - GigE Vision Control/Streaming Protocols
- **GenCP** - Generic Control Protocol
- **PFNC/SFNC** - Pixel Format and Standard Feature Naming Conventions
