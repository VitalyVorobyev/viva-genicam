<p align="center">
  <img src="assets/viva-studio-logo-and-text_opt.svg" alt="viva genicam" width="360">
</p>

# Viva Studio — developer notes

Viva Studio is the desktop application for GenICam cameras: browse a camera's
feature tree, change settings, watch the live image, and save a diagnostic
report. **Using it?** Download a release and read the user guide's
[Viva Studio](https://vitalyvorobyev.github.io/viva-genicam/studio.html)
chapter. This page is for working on it.

It is a React 19 + Tauri v2 app that lives in this repository as a **second
Cargo workspace** under `studio/`, so the published library crates stay free
of GUI and Node toolchain concerns. The working agreement for contributors is
in [`CLAUDE.md`](CLAUDE.md), the repository's in
[`../CONTRIBUTING.md`](../CONTRIBUTING.md); the Zenoh wire contract is in
[`../docs/studio/zenoh-api.md`](../docs/studio/zenoh-api.md), and what is
planned in the ST rows of [`../docs/backlog.md`](../docs/backlog.md).

---

## Layout

```
studio/
  crates/
    viva_xml_model/   GenICam XML -> UiGraph (the contract the UI consumes)
    viva_streamer/    Frame -> BMP encoding + WebSocket serving
  apps/
    viva-studio-tauri/  The desktop app (Tauri v2 shell + Rust backend)
  ui/
    viva-studio-ui/   React 19 + Vite front end
```

Without a camera, run the app against `viva-fake-gige` through `viva-service`
(`docs/studio/testing-cookbook.md`). The backend's end-to-end tests drive both
modes against the same fake, in-process.

`apps/viva-studio-tauri/src-tauri` is **excluded** from the studio Cargo
workspace — it is built with `cargo tauri`, not `cargo build --workspace`. It
has its own CI job (`tauri-lint` in `.github/workflows/studio-ci.yml`); a
`cargo clippy --workspace` in `studio/` will not touch it.

Key invariants:

- **Parsing lives in Rust.** The UI renders a contract, it does not read
  GenICam tags.
- **The UI depends on `UiGraph`**, not on the XML.
- **Nothing is dropped silently**: a declaration the library cannot build is
  listed in `UiGraph.skipped`, and a node kind the mapper does not know is
  kept as `Unknown`.

## Data model: `UiGraph`

The minimal model the UI needs, from `crates/viva_xml_model`:

- `nodes_by_name` — GenICam `Name` → `UiNode`
- `categories` — category name → `UiCategory` (with `features: string[]`)
- `root_category` — the chosen root, preferring `"Root"` when present

- `skipped` — declarations the library could not turn into features, with
  each one's tag, name and reason (omitted when empty)

The TypeScript types are generated from `crates/viva_xml_model/src/model.rs`
into `ui/viva-studio-ui/src/generated/ipc.ts` (backlog ST-28); a test fails
while that file is stale.

---

## Build and run from source

### Prerequisites

- Rust stable
- [Bun](https://bun.sh) for the UI
- Tauri CLI v2: `cargo install tauri-cli --version '^2'`

### Run the desktop app

```sh
cd studio/apps/viva-studio-tauri
cargo tauri dev
```

`beforeDevCommand` starts the Vite dev server for you; `tauri.conf.json` points
the shell at `http://localhost:5183`.

### Run the UI alone

```sh
cd studio/ui/viva-studio-ui
bun install
bun run dev      # bun run test / bun run build
```

### End-to-end with a fake camera

No hardware needed. Three terminals for GigE:

```sh
# 1: fake camera (from the repository root)
cargo run -p viva-fake-gige

# 2: the Zenoh bridge — --zenoh-config is required on the service side
cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5
# On Linux: --iface lo

# 3: the app. ZENOH_CONFIG is required, and must be absolute — the `cd` below
# means a relative path would no longer resolve.
export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri && cargo tauri dev
```

USB3 Vision needs only two, because the service hosts its own fake:

```sh
cargo run -p viva-service-u3v -- --fake --zenoh-config studio/config/zenoh-local.json5

export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri && cargo tauri dev
```

**`ZENOH_CONFIG` is what puts the app in remote mode.** There is no default
path and no dev-mode auto-detection: without it the app starts in *embedded*
mode and talks to cameras directly, which is a perfectly good mode but not this
one — embedded discovery does not scan loopback, so a fake camera on
`127.0.0.1` never appears and the device list stays empty. The mode is shown in
the app header, and a `ZENOH_CONFIG` that fails to load is reported as an error
rather than silently ignored.

## Building

```sh
cd studio/ui/viva-studio-ui && bun install
cd ../../apps/viva-studio-tauri && cargo tauri build
```

`cargo tauri build` builds the UI first, then this platform's bundles into
`apps/viva-studio-tauri/src-tauri/target/release/bundle/`. The release
workflow (`.github/workflows/release.yml`) does the same on macOS, Ubuntu and
Windows for every `v*` tag, and on any pull request that changes the
packaging.

---

## Using `viva_xml_model` as a library

```rust
use viva_xml_model::parse_genicam_xml;

let xml = std::fs::read_to_string("camera.xml")?;
let graph = parse_genicam_xml(&xml)?;
println!("root category: {}", graph.root_category);
```

This crate is Studio's own lightweight view of a GenApi document. For a full
NodeMap with evaluation, addressing and access predicates, use `viva-genapi`
from the main workspace — `NullIo` makes it work offline too.

---

## Quality gates

Run from `studio/`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

and, because `src-tauri` is outside that workspace:

```sh
cd apps/viva-studio-tauri/src-tauri
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

plus the UI:

```sh
cd ui/viva-studio-ui
bun install --frozen-lockfile && bunx tsc --noEmit && bun run lint && bun run test -- --run && bun run build
```

`bun run lint` is [Oxlint](https://oxc.rs), configured in `.oxlintrc.json` with
an explicit rule list: ESLint's and typescript-eslint's recommended rules
(all but `no-octal`, which `tsc` already rejects) and the React hooks and React
Compiler rules. Oxlint has no `config` or `gating` rule; those check React
Compiler options this app does not set.
