# ADR-0001: Desktop-primary with WASM maintenance mode

**Status:** Partly superseded — the decision holds, the WASM runtime does not
**Date:** 2026-03-06

> **What changed.** Desktop-primary is still the rule and still correct. But the
> browser runtime this ADR describes no longer exists: `genicam_xml_model_wasm`
> and `WebWasmProvider` were not carried into this repository at the monorepo
> import ([ADR-0017](adr0017-studio-monorepo-two-workspaces.md)), and there is no
> WASM job in CI. What survives of the idea is narrower and lives one layer down:
> `viva-genapi-xml` and `viva-genapi` compile for `wasm32-unknown-unknown` and
> `NullIo` lets a caller browse a GenApi XML with no camera attached, so a
> browser-side XML explorer remains buildable by anyone who wants one. The GenTL
> premise in the Context below was retired by
> [ADR-0011](adr0011-pure-rust-genicam-stack.md).

## Context

GenICam Studio has two runtime paths: a Tauri v2 desktop app and a browser-only WASM mode. The browser mode loads `genicam_xml_model_wasm` to parse XML client-side, while the desktop app uses native Rust parsing via IPC.

The target users are machine vision engineers who need to interact with physical cameras. Camera interaction requires native OS access (GenTL providers, USB/GigE), which only the desktop app can provide.

## Decision

- **Desktop (Tauri v2) is the primary and only supported runtime for camera interaction.**
- The WASM crate and `WebWasmProvider` remain in the codebase and are maintained for offline XML exploration and demo purposes.
- WASM stays in CI but is not a blocking gate for releases.
- New camera-interaction features (device discovery, node read/write, acquisition, image viewing) are implemented for Tauri only.

## Consequences

- No need to design browser-compatible alternatives for native features (Zenoh, GenTL, file system access).
- WASM path is a read-only XML explorer; it will not grow new capabilities.
- Reduced testing matrix for new features (desktop only).
