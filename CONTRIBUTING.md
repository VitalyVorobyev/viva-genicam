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
uv pip install --python .venv/bin/python maturin numpy pytest mypy
uv run --python .venv/bin/python maturin develop -m crates/viva-pygenicam/Cargo.toml
.venv/bin/python -m pytest crates/viva-pygenicam/tests
.venv/bin/python -m mypy.stubtest viva_genicam --allowlist crates/viva-pygenicam/stubtest-allowlist.txt
```

The pytest suites start their own fake cameras on UDP 3956, like the Rust
ones, and `test_book_snippets.py` runs the book's marked Python snippets
against one. `stubtest` checks the hand-written `_native.pyi` against the built
module, and the package's own code against both; run it from outside
`crates/viva-pygenicam`, so `viva_genicam` is the installed package.

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
cargo test -p viva-genicam --test gige_session       # control release, monitors, PENDING_ACK, discovery, multicast monitors
cargo test -p viva-genicam --test facade             # standard-feature helpers, both fake XML profiles (SFNC 2 and older names)
cargo test -p viva-genicam --test blocking           # the blocking API, and a camera opened from a current-thread runtime
cargo test -p viva-genicam --test persistence        # saving and restoring a configuration (ADR-0028)
cargo test -p viva-genicam --test refresh            # values the camera changes on its own: NoCache, polling, invalidate (ADR-0029)
cargo test -p viva-camctl  --lib field_engineer      # features, describe, dump, save/load, registers, action, USB3 Vision
cargo test -p viva-gige    --test status_codes       # GVCP status codes from literal acknowledgements (ADR-0020)
# Control-channel keepalive, against fixtures that withhold or delay ACKs
cargo test -p viva-genicam --test heartbeat          # idle keepalive, with a negative control
cargo test -p viva-genicam --test inflight_keepalive # probes continue while register I/O holds the mutex (against the fake)
cargo test -p viva-genicam --test keepalive_shutdown # dropping the transport cancels keepalive I/O
cargo test -p viva-gige    --test inflight_keepalive # heartbeats during long ACK waits (ADR-0023)
# GVSP receive engine, from scripted datagrams on ephemeral ports (no UDP 3956)
cargo test -p viva-genicam --test gvsp_engine        # reordering, loss, resend, timeouts, counters (ADR-0025)
# Chunk and event features through GenApi ports, from the fake's encoder (no sockets)
cargo test -p viva-genicam --test chunk_features     # ADR-0026
cargo test -p viva-genapi  --test ports              # port routing, attach/detach, invalidation
cargo test -p viva-genapi  --test persistence        # the save plan, the restore order, the file layout
cargo test -p viva-genapi  --test caching            # Cachable, PollingTime, changed_since and poll, on a scripted device
cargo test -p viva-gige    --test chunk_layout       # the chunk layout, from hand-written bytes
# USB3 Vision
cargo test -p viva-genicam --test fake_u3v_camera --features u3v  # against viva-fake-u3v: transfers in flight, stalls, unplug, chunks, events, time
cargo test -p viva-u3v     --test gencp_conformance  # wire layouts from GenCP 1.2 and vendor XML
cargo test -p viva-u3v     --lib stream              # the stream engine against scripted transfers: replanning, recovery, disconnect
# Services
cargo test -p viva-service --test fake_camera_e2e    # viva-service end to end over Zenoh
cargo test -p viva-service-u3v                       # the USB3 Vision service over Zenoh: serving, cameras coming and going
# GenTL producer (ADR-0031), its C functions called in process, against viva-fake-gige
cargo test -p viva-gentl --test lifecycle            # GCInitLib/GCCloseLib, open and close, closing cascades
cargo test -p viva-gentl --test enumeration          # interfaces, devices, info commands, access modes
cargo test -p viva-gentl --test ports                # each module's XML and registers; the remote port
cargo test -p viva-gentl --test acquisition          # announce, queue, deliver, flush, incomplete frames, chunks
cargo test -p viva-gentl --test soundness            # invalid handles, null pointers, panics, per-thread errors
cargo test -p viva-gentl --test logging              # RUST_LOG makes the producer log
# Documentation
cargo test -p viva-genicam --test book_includes      # every code include in the book resolves
cargo test -p viva-genicam --test doc_links          # every relative Markdown link resolves
```

The GenTL producer is also driven as consumers load it: `scripts/gentl-harness.sh`
builds `viva_gentl.cti` (`scripts/build-cti.sh`), compiles a C program against
the EMVA's `GenTL.h` — fetched by `scripts/fetch-gentl-header.sh`, not
committed — and acquires frames from `viva-fake-gige`, then validates every
port's XML with `xmllint` against the GenApi 1.1 schema. With
`HARVESTERS_PYTHON` naming a Python that has `harvesters==1.4.3`,
`genicam==1.6.0` and `numpy` installed, it also acquires through Harvesters
(`scripts/gentl-harvesters.py`), a consumer we did not write. CI's `GenTL
producer` job runs all three on Linux; `GENICAM_PACKAGE_ZIP` reuses a downloaded
EMVA package.

A real USB3 Vision camera has a smoke suite of its own, which neither
`cargo test --workspace` nor CI runs: every test is `#[ignore]`d. With a camera
plugged in, it discovers, opens, streams, reads chunks and events, restarts a
stream and fits the clock, reads string and `<pLength>` features, tries an
enumeration entry the camera does not offer, and saves and loads its
configuration; it puts back what it changed and writes nothing non-volatile.
Point the camera at a lit scene: the stream test fails on a flat frame. One
test at a time, since a USB device admits one owner; `VIVA_U3V_INDEX` picks a
camera other than the first:

```bash
cargo test -p viva-genicam --features u3v-usb --test u3v_hardware -- --ignored --test-threads=1
```

One camera has run it (ADR-0032 §8). Run it with any other, and
attach the output to the pull request or issue.

A real GigE Vision camera has one too, `crates/viva-genicam/tests/gige_hardware.rs`,
the parts of the [hardware test plan](docs/hardware-test-plan.md) a test can
judge: discovery by broadcast and by address; control, monitor and exclusive
access, and a second application refused while one holds control; every
readable node read, every writable feature given its own value back, every
enumeration entry set; commands reporting done; the standard-feature helpers,
the region of interest, binning and decimation; a software trigger at a steady
rate; a stream in every pixel format, at an odd width in each packed one, and
across a hundred acquisition restarts; a resend request from the stream's
socket; chunks, events and a reader a second late; action commands; the fitted
clock and its reset; and a save and load round trip. `VIVA_GIGE_IP` names the camera
and `VIVA_GIGE_IFACE` the host interface (by name or address; without it, the
one routed to the camera). Each test loads the camera's configuration back at
the end; non-volatile state is written, unchanged, only with
`VIVA_GIGE_ALLOW_PERSISTENT=1`. A feature the camera lacks is skipped with a
message. Run no fake-camera suite meanwhile (UDP 3956), and one test at a
time, since a camera takes one controller:

```bash
VIVA_GIGE_IP=<CAM> VIVA_GIGE_IFACE=<NIC> \
  cargo test -p viva-genicam --test gige_hardware -- --ignored --test-threads=1 --nocapture
```

Attach its output, which prints what each test found, to the pull request or
issue.

GVSP parsing and block assembly also have fuzz targets, in `fuzz/` (outside
the workspace; needs nightly and `cargo install cargo-fuzz`):
`cargo +nightly fuzz run gvsp_parse`, `gvsp_assemble` or `chunk_parse`.

Every fake-camera suite binds UDP port 3956, so two test processes running at
once — in two checkouts, say — interfere and fail spuriously. Run them one at a
time. For logs: `RUST_LOG=debug cargo test --workspace -- --nocapture`.

**Fakes must be able to disagree with the client.** A fake written from the
same reading as the code it tests only proves the two agree; ours are built
from the specifications
([ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md)).
They import the transport crates' vocabulary — register addresses, opcodes,
status codes — but encode and decode every packet and register themselves, so
they can still disagree about what lives at an address
([ADR-0030](docs/adrs/adr0030-errors-and-public-surface.md)).
A fix starts with a test that fails without it.

### Vendor XML corpus

Three further tests parse, evaluate and use real vendor GenApi documents. The
documents are vendor copyright, so they are fetched rather than committed, and
the tests do nothing when the directory is absent:

```bash
scripts/fetch-xml-corpus.sh
cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture   # parses
cargo test -p viva-genapi     --test vendor_corpus -- --nocapture   # + evaluates, plans each save
cargo test -p viva-genicam    --test sfnc_corpus   -- --nocapture   # + the Camera helpers' features
```

`VIVA_GENICAM_XML_CORPUS` points them at another directory — XML dumped from
your own camera with `viva-camctl xml`, for instance.

The parse test also fails on a child element the parser reads past without
interpreting (`XmlModel::ignored`) unless `EXPECTED_IGNORED` allows it, with
the reason: documentation, or a known gap's backlog row. A new vendor
document that brings a new element fails it until the element is
implemented or allowed.

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
  The Python snippets are hand-written: put `<!-- pytest: camera -->` above
  one (or `device`, or `none`; `test_book_snippets.py` says what each
  provides) and pytest runs it against the fake camera.
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
