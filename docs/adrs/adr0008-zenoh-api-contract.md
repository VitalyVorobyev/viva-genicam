# ADR-0008: Zenoh key-expression API contract

**Status:** Accepted
**Date:** 2026-03-06

## Context

The Tauri desktop app and the camera service communicate over Zenoh. The API is defined in `docs/studio/zenoh-api.md` and typed in `crates/viva-zenoh-api/`.

## Decision

The Zenoh API contract is the **single integration boundary** between the camera service and GenICam Studio. All communication flows through typed Zenoh key expressions.

### Key principles
- **Service-owned data flows outward**: the service publishes `announce`, `status`, `nodes/*/value`, `acquisition/status`, and `image`. The app never writes to these keys.
- **App requests flow inward**: the app uses Zenoh `get()` to `xml`, `nodes/{name}/set`, `nodes/{name}/execute`, and `acquisition/control`.
- **Types are shared**: the `viva-zenoh-api` crate defines all payload types as `Serialize + Deserialize` structs with no Zenoh dependency, so both sides use the same types.
- **Key helpers are centralized**: `viva_zenoh_api::keys` module provides all key-expression builders.

### Versioning
- The API is versioned by the `viva-zenoh-api` crate version.
- Breaking changes bump the crate minor version (pre-1.0).
- The app and service must use compatible `viva-zenoh-api` versions.

## Consequences

- Adding new features (e.g., new node types, events) requires updating the shared crate.
- The API is transport-agnostic at the payload level; Zenoh handles routing and discovery.
- Future: if the camera library is embedded directly, the Zenoh API types can still be used as in-process message types.
