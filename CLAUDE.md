# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

genicam-rs is a pure Rust implementation of GenICam ecosystem building blocks with an Ethernet-first focus (GigE Vision). It provides libraries and CLI tools for camera discovery, control, streaming, and feature access.

We do not maintain backward compatibility at this early development stage. The priority is clear design and structure.

## Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Format check (CI requirement)
cargo fmt --all --check

# Linting (CI runs with warnings-as-errors)
cargo clippy --workspace --all-targets -- -D warnings

# Generate docs
cargo doc --workspace --all-features --no-deps

# Run a specific example
cargo run -p genicam --example list_cameras

# Run CLI tool
cargo run -p gencamctl -- list
```

## Architecture

**Layered design (bottom to top):**

```
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

## Key Abstractions

**`RegisterIo` trait** (`genapi-core`): Core abstraction for register read/write. Implemented by `GigeDevice` (via async adapter) and `MockIo` for tests.

**`NodeMap`** (`genapi-core`): Parsed from XML, stores nodes by name, tracks dependency graph for cache invalidation. Methods: `get_integer()`, `set_float()`, `get_enum()`, `exec_command()`.

**`Node` enum**: Integer, Float, Enum, Boolean, Command, Category, SwissKnife (computed expressions).

**`GigeDevice`** (`tl-gige`): Async UDP wrapper for GVCP discovery/control and GVSP streaming.

## Testing

Tests are embedded in source modules (`mod tests { }`), not in a separate `tests/` directory. Use `MockIo` implementing `RegisterIo` for transport abstraction.

```bash
# Test single crate
cargo test -p genapi-core

# Test with logging
RUST_LOG=debug cargo test --workspace -- --nocapture
```

## Documentation

- **mdBook**: `book/` directory - tutorials, architecture, networking cookbook
- **API docs**: Generated via `cargo doc`, published to GitHub Pages
- **Examples**: 17 examples in `crates/genicam/examples/`

## Standards

This codebase implements these EMVA standards:
- **GenApi** - XML-based node description (Tier-1 + SwissKnife)
- **GVCP/GVSP** - GigE Vision Control/Streaming Protocols
- **GenCP** - Generic Control Protocol
- **PFNC/SFNC** - Pixel Format and Standard Feature Naming Conventions
