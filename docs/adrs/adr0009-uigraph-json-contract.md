# ADR-0009: UiGraph as the single UI data contract

**Status:** Accepted
**Date:** 2026-03-06

## Context

The UI needs to render camera feature trees. The `UiGraph` type bridges Rust parsing and TypeScript rendering.

## Decision

`UiGraph` is the **single data contract** between all backends (WASM, Tauri IPC, future embedded) and the React UI. The UI never parses XML or interacts with camera SDKs directly.

### Contract location
- Rust: `crates/genicam_xml_model/src/model.rs`
- TypeScript: `studio/ui/viva-studio-ui/src/xml_model/uigraph.ts`

### Stability rules
- Fields can be added (with `Option` / `?` and `skip_serializing_if`).
- Fields must not be removed or renamed without explicit request.
- `UiNodeKind` variants can be added but not removed.
- `RawNode` is always populated for every node (preservation invariant).

### Provider abstraction
The `XmlModelProvider` interface (`provider.ts`) abstracts the backend:
- `WebWasmProvider`: loads WASM, parses XML client-side, returns `UiGraph`.
- `TauriProvider`: calls Tauri IPC commands, returns `UiGraph` + live node operations.

## Consequences

- All UI components are backend-agnostic.
- New node kinds or features only require updating the contract and the providers.
- Snapshot tests validate the contract against known XML fixtures.
