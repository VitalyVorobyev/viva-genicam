# CLAUDE.md — Viva Studio

Viva Studio is a Tauri v2 + React 19 GUI for GenICam cameras, and the
library's reference viewer (ADR-0024). It drives a camera in-process through
viva-genicam (embedded mode, the primary path) or consumes viva-service /
viva-service-u3v over Zenoh (remote mode). It is the **second Cargo
workspace** of this monorepo (kept separate from the published library
workspace at the repo root — see ADR-0017).

## Pointers

- Root `CLAUDE.md` — repo-wide rules (pre-push gates, version bumps).
- `docs/design.md` — overall architecture; `docs/backlog.md` — ST epic.
- `docs/studio/` — the Zenoh API contract (`zenoh-api.md`), the testing
  cookbook, and the manual E2E checklist.

## Build & Run

```bash
# Rust workspace (from studio/)
cargo build && cargo test --workspace

# UI dev server
cd studio/ui/viva-studio-ui && bun install && bun run dev

# Tauri dev
cd studio/apps/viva-studio-tauri && cargo tauri dev
```

Against the in-repo fake camera, follow `docs/studio/testing-cookbook.md`:
`ZENOH_CONFIG` is required there and must be absolute.

## Key Invariants

- Zenoh payload types come ONLY from `viva-zenoh-api` (which must not
  depend on zenoh itself).
- XML parsing happens only in Rust crates (`viva_xml_model` on top of
  `viva-genapi-xml`). The UI consumes UiGraph JSON and never parses XML.
- UiGraph contract fields `nodes_by_name` / `categories` / `root_category`
  are frozen.
- No declaration is dropped silently: one the library cannot build is listed
  in `UiGraph.skipped` with its tag, name and reason, and a
  `NodeDecl` kind the mapper does not know becomes `UiNodeKind::Unknown`.
- In the Tauri backend use `tauri::async_runtime::spawn`, never bare
  `tokio::spawn`. Reach the embedded camera only through
  `EmbeddedBackend::with_camera`, which runs the operation on the blocking
  pool: the camera's register I/O blocks its thread.
- The UI's IPC types — payloads, the UiGraph contract and the `StudioEvents`
  map — are generated from the Rust types into
  `ui/viva-studio-ui/src/generated/ipc.ts`; never edit that file. After
  changing a type the UI receives, run `UPDATE_BINDINGS=1 cargo test bindings`
  in `apps/viva-studio-tauri/src-tauri` and commit the result: a plain
  `cargo test` there fails while the file is stale. Emit events through
  `Events::send` with a `StudioEvent` type, and listen with `listenTo`
  (`ui/.../device/events.ts`).
- The UI has one cache of live feature state, `FeatureStore`
  (`ui/.../device/featureStore.ts`). Read a feature with `useFeature(name)`,
  write with `useFeatureWriter`, and draw one with `<FeatureControl>`; never
  keep a second copy. The store reads only what is subscribed — what is on
  screen — and re-reads it when a write invalidates it, and at its polling
  time (`refreshInterval`, ADR-0029) while it stays on screen.
- Invoke native commands through `invokeCommand` (`ui/.../device/commands.ts`);
  its names, wire arguments and success types come from Rust signatures via
  the bindings generator. Do not call raw Tauri `invoke` elsewhere.
- A command registered in `main.rs` needs a permission in
  `src-tauri/permissions/` that `capabilities/main.json` grants: Tauri refuses
  it otherwise, and only in the running app. `src/permissions.rs` checks the
  sets against the registered commands and the UI's `invoke` calls.
- Package manager is bun, never npm (`bun.lock` is authoritative).
- `apps/` stays thin glue (windowing, dialogs, command wiring) — no
  parsing or business logic.
