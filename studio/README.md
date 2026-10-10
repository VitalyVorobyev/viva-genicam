<p align="center">
  <img src="assets/viva-studio-logo-and-text_opt.svg" alt="viva genicam" width="360">
</p>

# Viva Studio

Viva Studio is the desktop application for GenICam cameras: browse a camera's
feature tree, change settings, and watch the live image. It is a React 19 + Tauri
v2 app that lives in this repository as a **second Cargo workspace** under
`studio/`, so the published library crates stay free of GUI and Node toolchain
concerns.

> **Experimental.** Studio works — it discovers cameras, reads and writes
> features, and streams live frames. But it is the least-exercised part of this
> repository: it has no hardware in CI, and its hardware reports come from a few
> users and camera models. Treat what it displays as unconfirmed
> until you have seen it agree with your camera. Bug reports are very welcome.

The working agreement for contributors is in [`CLAUDE.md`](CLAUDE.md); the Zenoh
wire contract is in [`../docs/studio/zenoh-api.md`](../docs/studio/zenoh-api.md).

---

## What works today

- **Live cameras.** Discovery, connect, feature read/write, and GVSP streaming,
  either through the embedded backend (`viva-genicam` linked directly into the
  Tauri app) or over Zenoh from `viva-service`.
- **Feature browser.** Category tree, search, typed editors, live values, and
  access-mode handling — a read-only or locked feature is presented as such.
- **Offline XML browsing.** Open a GenApi XML file with no camera attached and
  explore the same tree.
- **Unknown-node visibility.** Unrecognised XML tags become `Unknown` nodes
  rather than disappearing, and declarations the library could not turn into
  features are listed in the Diagnostics tab and counted in the toolbar.
- **A diagnostic report.** The Diagnostics tab saves the report
  `viva-camctl report` writes, for attaching to an issue.

## What is not there yet

- **USB3 Vision discovery in the embedded backend** — it reads only the GigE
  cache, though `u3v-usb` is compiled in (backlog ST-14). The Zenoh path via
  `viva-service-u3v` does work.
- **Signed builds.** The release bundles are unsigned; see
  [Install a release build](#install-a-release-build).
- **Recording and playback**, **frame annotation**, **auto-update** — planned,
  see `../docs/backlog.md`.

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
- **Unknown nodes are preserved** rather than dropped.

## Data model: `UiGraph`

The minimal model the UI needs, from `crates/viva_xml_model`:

- `nodes_by_name` — GenICam `Name` → `UiNode`
- `categories` — category name → `UiCategory` (with `features: string[]`)
- `root_category` — the chosen root, preferring `"Root"` when present

Each `UiNode` carries a `RawNode` snapshot: the original element `tag`, its
`attributes` (always including `Name` when present) and `children_text`, with
repeated fields such as `pFeature` joined by newlines. The TypeScript copy of
the contract is `ui/viva-studio-ui/src/xml_model/uigraph.ts` and must stay in
step with `crates/viva_xml_model/src/model.rs`.

---

## Install a release build

Releases after 0.7.0 attach Viva Studio to the
[GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases),
versioned with the library, as `viva-studio-<tag>-<target>.<ext>`:

| OS | File | Install |
|---|---|---|
| macOS, Apple Silicon | `.dmg` | Open it and drag Viva Studio to Applications |
| Linux x86_64 | `.deb` or `.AppImage` | `sudo apt install ./viva-studio-….deb`, or `chmod +x` the AppImage and run it |
| Windows x86_64 | `.msi` | Run the installer |

The Linux builds are made on Ubuntu 22.04 and need its glibc or newer, and
WebKitGTK 4.1 (the `.deb` pulls it in). Running an AppImage needs FUSE 2
(`libfuse2t64` on Ubuntu 24.04). `SHA256SUMS.txt` on the release lists every
file's checksum.

**The bundles are not signed** — that needs paid certificates from Apple and
Microsoft — so the first launch asks for confirmation:

- **macOS** refuses to open the app the first time. Open System Settings →
  Privacy & Security and choose *Open Anyway* for Viva Studio, or clear the
  download's quarantine flag:
  `xattr -dr com.apple.quarantine "/Applications/Viva Studio.app"`.
- **Windows** SmartScreen shows "Windows protected your PC": choose
  *More info*, then *Run anyway*. If Windows Firewall then asks whether Viva
  Studio may use the network, allow the network your camera is on: discovery
  and streaming arrive as inbound UDP.

## Quickstart (from source)

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
