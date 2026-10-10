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
- `docs/studio/` — Zenoh API contract, camera-service API, testing
  cookbook, manual E2E checklist.

## Build & Run

```bash
# Rust workspace (from studio/)
cargo build && cargo test --workspace

# UI dev server
cd studio/ui/viva-studio-ui && bun install && bun run dev

# Tauri dev
cd studio/apps/viva-studio-tauri && cargo tauri dev

# E2E against the in-repo fake camera (3 terminals, repo root)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5
# T3: export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
#     cd studio/apps/viva-studio-tauri && cargo tauri dev
```

`ZENOH_CONFIG` on T3 is required and must be absolute (the `cd` breaks a
relative path). Without it the app starts in embedded mode, whose discovery
skips loopback — so the fake camera never appears. See `#132`.

## Key Invariants

- Zenoh payload types come ONLY from `viva-zenoh-api` (which must not
  depend on zenoh itself).
- XML parsing happens only in Rust crates (`viva_xml_model` on top of
  `viva-genapi-xml`). The UI consumes UiGraph JSON and never parses XML.
- UiGraph contract fields `nodes_by_name` / `categories` / `root_category`
  are frozen.
- Unknown XML nodes are never dropped: preserved as `UiNodeKind::Unknown`.
  `RawNode` is meant to carry their XML but is empty today (backlog ST-34).
- In the Tauri backend use `tauri::async_runtime::spawn`, never bare
  `tokio::spawn`.
- The UI's IPC types — payloads, the UiGraph contract and the `StudioEvents`
  map — are generated from the Rust types into
  `ui/viva-studio-ui/src/generated/ipc.ts`; never edit that file. After
  changing a type the UI receives, run `UPDATE_BINDINGS=1 cargo test bindings`
  in `apps/viva-studio-tauri/src-tauri` and commit the result: a plain
  `cargo test` there fails while the file is stale. Emit events through
  `Events::send` with a `StudioEvent` type, and listen with `listenTo`
  (`ui/.../device/events.ts`).
- Package manager is bun, never npm (`bun.lock` is authoritative).
- `apps/` stays thin glue (windowing, dialogs, command wiring) — no
  parsing or business logic.
