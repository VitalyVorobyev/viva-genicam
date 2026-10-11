# Contributing to viva-genicam

Thanks for helping. This page is for people changing viva-genicam; if you are
*using* it, the [user guide](https://vitalyvorobyev.github.io/viva-genicam/) is
the place to start.

The most valuable contribution needs no code at all: a report from a camera we
have not seen. `viva-camctl report` writes everything we need to diagnose one
— see [Reporting a camera we can't open](https://vitalyvorobyev.github.io/viva-genicam/reporting.html).

## Where things are

| Path | What |
|---|---|
| `crates/` | The library workspace: protocol crates, the `viva-genicam` facade, the services, `viva-camctl`, the fake cameras |
| `crates/viva-pygenicam/` | The Python bindings — a workspace of its own, with its own `Cargo.lock` |
| `studio/` | Viva Studio, a second workspace; its Tauri app (`studio/apps/viva-studio-tauri/src-tauri`) is a third ([`studio/README.md`](studio/README.md)) |
| `book/` | The user guide (mdBook), published to GitHub Pages |
| `docs/` | Developer documentation: design, standards, decisions, roadmap and backlog ([`docs/README.md`](docs/README.md)) |

The crate map, the layering, the key abstractions and the design tenets are in
[`docs/design.md`](docs/design.md); the GenICam standards, and which crate
implements which, in [`docs/standards.md`](docs/standards.md).

## Build and check

You need Rust 1.88 or newer (edition 2024). For Studio, also
[bun](https://bun.sh) and Node 22.22.2 or newer.

Before pushing, run what CI runs. For the library workspace:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo test --workspace
```

The Python bindings are a separate workspace, and so is the Tauri app:

```bash
(cd crates/viva-pygenicam && cargo clippy --all-targets -- -D warnings && cargo fmt --check)
(cd studio/apps/viva-studio-tauri/src-tauri && cargo clippy --all-targets -- -D warnings)
```

To build the Python package from source and run its tests (with
[uv](https://docs.astral.sh/uv/)):

```bash
uv venv .venv
uv pip install --python .venv/bin/python maturin numpy pytest
uv run --python .venv/bin/python maturin develop -m crates/viva-pygenicam/Cargo.toml
.venv/bin/python -m pytest crates/viva-pygenicam/tests
```

Run the Tauri crate's clippy whenever a public type in the library changes,
not only when `studio/` does: a new enum variant can break it with nothing else
local noticing. Studio's own checks are in [`studio/README.md`](studio/README.md#quality-gates).

## Tests

Everything runs without hardware or external tools: the integration tests
start in-process fake cameras (`viva-fake-gige`, `viva-fake-u3v`) on loopback.
`cargo test --workspace` runs all of them; the suites can also run alone:

```bash
# GigE Vision, against viva-fake-gige
cargo test -p viva-genicam --test fake_camera        # discovery, features, streaming
cargo test -p viva-genicam --test predicates         # pIsLocked, pIsAvailable, ... end to end
cargo test -p viva-genicam --test register_access    # which GVCP command a register access sends
cargo test -p viva-genicam --test report             # the diagnostic report every tool writes
# Control-channel keepalive, against fixtures that withhold or delay ACKs
cargo test -p viva-genicam --test heartbeat          # idle keepalive, with a negative control
cargo test -p viva-genicam --test inflight_keepalive # probes continue while register I/O holds the mutex
cargo test -p viva-genicam --test keepalive_shutdown # dropping the transport cancels keepalive I/O
cargo test -p viva-gige    --test inflight_keepalive # heartbeats during long ACK waits (ADR-0023)
# USB3 Vision
cargo test -p viva-genicam --test fake_u3v_camera --features u3v  # against viva-fake-u3v
cargo test -p viva-u3v     --test gencp_conformance  # wire layouts from GenCP 1.2 and vendor XML
# Services
cargo test -p viva-service --test fake_camera_e2e    # viva-service end to end over Zenoh
# Documentation
cargo test -p viva-genicam --test book_includes      # every code include in the book resolves
cargo test -p viva-genicam --test doc_links          # every relative Markdown link resolves
```

Every fake-camera suite binds UDP port 3956, so two test processes running at
once — in two checkouts, say — interfere and fail spuriously. Run them one at a
time. For logs: `RUST_LOG=debug cargo test --workspace -- --nocapture`.

**Fakes must be able to disagree with the client.** A fake written from the
same reading as the code it tests only proves the two agree; ours are built
from the specifications, with constants of their own
([ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md)).
A fix starts with a test that fails without it.

### Vendor XML corpus

Two further tests parse and evaluate real vendor GenApi documents. The
documents are vendor copyright, so they are fetched rather than committed, and
the tests do nothing when the directory is absent:

```bash
scripts/fetch-xml-corpus.sh
cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture   # parses
cargo test -p viva-genapi     --test vendor_corpus -- --nocapture   # + evaluates
```

`VIVA_GENICAM_XML_CORPUS` points them at another directory — XML dumped from
your own camera with `viva-camctl xml`, for instance.

## Evidence

When sources disagree about what a camera does, weigh them in this order: real
hardware, then the specification, then vendor XML, then other implementations
such as aravis. The full rule, and how it applies to what we write in issues
and the changelog, is in [`CLAUDE.md`](CLAUDE.md#evidence-hierarchy).

## Documentation

- **The book** (`book/`) is for users. Its Rust snippets are included from the
  examples in `crates/viva-genicam/examples/` through `// ANCHOR:` markers, so
  they compile with the workspace; add a snippet by anchoring it in an example,
  never inline. `book_includes` fails on an include that does not resolve.
- **`docs/`** is for developers: design, ADRs, roadmap, backlog.
- **`doc_links`** fails on any relative link, in any Markdown file in the
  repository, that points at nothing.
- **One fact, one place.** Each audience has one canonical page for a fact —
  the book's [welcome page](book/src/welcome.md) for what works and what does
  not, [`book/src/architecture.md`](book/src/architecture.md) and
  [`docs/design.md`](docs/design.md) for the crate map — and other pages link
  to it rather than repeat it, so a change edits one file.

### The changelog

[`CHANGELOG.md`](CHANGELOG.md) is an index plus the `[Unreleased]` section;
each release series has its own file in [`changelog/`](changelog/). Entries are
for users:

- one bullet per user-visible change: what changed and why, in a sentence or
  two, with the pull request linked;
- no narrative, no test inventories, no internal refactors a user cannot see —
  the pull request holds those;
- a change that needs action on upgrade also goes in the release's opening
  "Read before upgrading" list.

## Pull requests

- Keep a pull request to one review surface; several related fixes in one PR
  are fine.
- Add an entry to `[Unreleased]` in [`CHANGELOG.md`](CHANGELOG.md), following
  [the changelog rules](#the-changelog).
- Refer to issues with "Addresses #N", not "Closes #N": an issue about a camera
  closes when its reporter confirms the fix on their hardware.
- An architectural decision gets an ADR in `docs/adrs/` (template in
  [`docs/adrs/README.md`](docs/adrs/README.md)).

## Extending the library

- A new GenApi node kind is a `Node` variant in `viva-genapi` with its
  evaluation and dependency wiring; anything unsupported lands in
  `NodeMap::skipped()` rather than failing the document.
- A transport is a `viva-*` crate behind `RegisterIo`, which the facade
  composes with a `NodeMap`.
- Keep `viva-genicam` thin: compose transport, node map and utilities there,
  and keep the logic in the crates below it.
