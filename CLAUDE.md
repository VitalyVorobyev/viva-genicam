# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

viva-genicam is a pure Rust implementation of GenICam ecosystem building blocks supporting GigE Vision and USB3 Vision. It provides libraries and CLI tools for camera discovery, control, streaming, and feature access.

We do not maintain backward compatibility at this early development stage. The priority is clear design and structure.

## Related Projects

- **Viva Studio** (`studio/`) — Tauri desktop app, second Cargo workspace in this repo (see `studio/CLAUDE.md`).
- **aravis** (`../aravis`) — C library for GenICam cameras. Optional; a corroborating second opinion when reading the wire protocols, not an authority on them (see [Evidence hierarchy](#evidence-hierarchy)). Not required for development or CI.

## Build Commands

```bash
# Build entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Integration tests (uses in-process fake camera, no external tools needed)
cargo test -p viva-genicam --test fake_camera

# Format check (CI requirement)
cargo fmt --all --check

# Linting (CI runs with warnings-as-errors)
cargo clippy --workspace --all-targets -- -D warnings

# Generate docs
cargo doc --workspace --all-features --no-deps

# Run sensor service
cargo run -p viva-service -- --iface en0

# Run CLI tool
cargo run -p viva-camctl -- list
```

## Pre-Push Checklist

Before pushing to a remote branch, always run these three gates locally
— CI runs them with warnings-as-errors and will reject any failure:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --all-features --no-deps
```

Transitive feature unification can mask breakage locally (a workspace
sibling may already pull a crate in with extra features), so clippy
and doc on a clean checkout matter — don't rely only on "it built on
my machine".

## Architecture

**Layered design (bottom to top):**

```
viva-service            - Zenoh bridge: GigE cameras → Viva Studio
viva-service-u3v        - Zenoh bridge: U3V cameras → Viva Studio
    ↓
viva-genicam (facade)   - End-user API: Camera<T>, discovery, streaming
    ↓
viva-genapi             - GenApi engine: NodeMap, node evaluation, caching
    ↓
viva-genapi-xml         - XML parsing: GenICam XML → XmlModel IR
    ↓
viva-gige / viva-u3v    - Transport: GVCP/GVSP for GigE, USB3 Vision
    ↓
viva-gencp              - Protocol primitives: GenCP encode/decode
```

Supporting crates: `viva-pfnc`, `viva-sfnc`, `viva-zenoh-api`, `viva-camctl`, `viva-pygenicam`; test crates: `viva-fake-gige`, `viva-fake-u3v`. Full crate map, layering rules, and data flows: [docs/design.md](docs/design.md).

## Key Abstractions

The core seams are the sync `RegisterIo` trait (`GigeRegisterIo`/`MockIo`/`NullIo`), `NodeMap` with pValue delegation and cache invalidation, `GigeDevice` (GVCP/GVSP), `FrameStream`/`U3vFrameStream`, and the services' `DeviceHandle`/`U3vDeviceHandle<T>`. Details: [docs/design.md](docs/design.md#key-abstractions).

## Design Principles

The design tenets live in [docs/design.md](docs/design.md#design-tenets). In one line: clear API boundaries, SOLID applied pragmatically, DRY, YAGNI, spec-conformance testing, error source chains. When a change conflicts with a tenet, write or update an ADR in `docs/adrs/` rather than silently deviating.

## Testing

Unit tests are embedded in source modules (`mod tests { }`). Integration tests use `viva-fake-gige` (in-process fake camera) and run automatically -- no external tools or hardware required.

```bash
# All tests (unit + integration + service e2e)
cargo test --workspace

# GigE integration tests (38: discovery, features, streaming)
cargo test -p viva-genicam --test fake_camera

# GenApi predicates against a real device (6: pIsLocked, pIsAvailable, ...)
cargo test -p viva-genicam --test predicates

# Control-channel keepalive (2: idle survival + a negative control)
cargo test -p viva-genicam --test heartbeat

# Which GVCP command leaves the host (6: READREG/WRITEREG dispatch, fallback)
cargo test -p viva-genicam --test register_access

# U3V integration tests (5: open, features, streaming, pixel formats)
cargo test -p viva-genicam --test fake_u3v_camera --features u3v

# Service end-to-end tests (5: acquisition, double-start, sustained streaming)
cargo test -p viva-service --test fake_camera_e2e

# Every fake-camera suite binds UDP 3956: two test processes running at once
# (e.g. in two worktrees) cross-talk and fail spuriously.

# Test with logging
RUST_LOG=debug cargo test --workspace -- --nocapture
```

### Vendor XML corpus

Fake cameras only exercise constructs we already thought of. Real vendor
GenApi XML is where the surprises live -- issues #45 and #35 were both a
single vendor construct making a camera unopenable, and both reached
users before us. The corpus test parses 38 documents: 35 real device
descriptions (AVT, Basler, Baumer, FLIR, Hikrobot, JAI, Micro-Epsilon,
PCO, Point Grey, Photonic Science, Prosilica, SVS, Sony, TIS), the
GenICam conformance document, and aravis's two synthetic devices.
Count them with `ls fixtures/vendor-xml/*.xml | wc -l` rather than
trusting this paragraph — it has drifted before, and three files here
are not vendor hardware. Most are fetched from third-party projects; the
Hikrobot MV-CS050-10GC (#35), five FLIR Blackfly / Blackfly S
descriptions (#45), the Micro-Epsilon scanCONTROL 850050 (#93) and the
TIS DMK 33GP2000e (#122) were contributed by users and are fetched from
the issue attachments or a pinned gist revision. The DMK is the only
document in the corpus that opens with a UTF-8 byte-order mark, which is
the defect it was reported for. The fifth FLIR document is the #45 reporter's
own BFS-PGE-31S4C-C, obtained with `viva-camctl xml` after 0.3.0 fixed
the defect that made it unopenable — so that exact model is now covered
directly rather than by four stand-ins.

**Not every contribution has to follow a bug.** The scanCONTROL is the
first profile scanner and the first Micro-Epsilon device in the corpus,
offered by a contributor who had no problem to report; it was asked for
while merging their unrelated PFNC patch (#93). Its four `<Register>`
nodes — one of them `FileAccessBuffer`, the same node a JAI skips —
are third-party evidence for `GA-09`, and they explain why that same
contributor needed `NodeMap::register_address` in #92.

```bash
scripts/fetch-xml-corpus.sh        # into fixtures/vendor-xml/ (gitignored)
cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture  # parses
cargo test -p viva-genapi     --test vendor_corpus -- --nocapture  # + evaluates
```

Parsing is only half the job. The `viva-genapi` stage builds a `NodeMap`
from each document and evaluates every node against a stub transport,
which is where the formula language, the address model and the numeric
codecs actually get exercised — every defect behind issue #35 lived
above the parser, invisible to the XML-layer test. See
[ADR-0018](docs/adrs/adr0018-genapi-conformance-over-convenience.md).

The documents are vendor copyright, published for interoperability by
third-party projects, so we fetch rather than redistribute them. The test
is a no-op when the directory is absent, so a fresh clone and PR CI stay
green; the `Vendor XML Corpus` workflow runs it weekly and on demand.

Point `VIVA_GENICAM_XML_CORPUS` at another directory to check XML dumped
from your own hardware. **When a user reports a camera we cannot open,
ask for their XML and add it to the corpus** -- that is how this class of
bug stops recurring. Ask for it by pointing them at the command rather
than at a code snippet:

```bash
viva-camctl report --ip <CAMERA-IP> --out viva-report.txt   # everything
viva-camctl xml    --ip <CAMERA-IP> --out camera.xml        # just the XML
```

Both stop before the nodemap, so they work on a camera we cannot open --
which is the only camera anyone reports. The snippet given in #45 needed
a successful connect and had to be retracted.

A node we cannot handle no longer fails the document. The XML layer drops
it into `XmlModel::skipped`; the GenApi layer drops it into
`NodeMap::skipped()`. Both are logged, and both corpus tests fail on any
skipped node not listed in their `EXPECTED_SKIPS` allowlist, so new gaps
surface instead of hiding.

**When the GenICam specification and a convenient approximation disagree,
implement the specification** rather than your own reading of it. ADR-0018
lists eight defects that each looked reasonable in isolation.

### Evidence hierarchy

When two sources disagree about what the wire or the XML means, weigh them
in this order:

1. **Real hardware.** A camera that a user actually owns is the only
   evidence that settles a question. Devices are often non-conformant,
   buggy, or inconsistent with their own documentation — and that is the
   point: **the goal is to work with the hardware that exists, not with
   the hardware the standard describes.** If a camera and the
   specification disagree, we accommodate the camera, and record why.
2. **The specification**, for anything hardware has not yet contradicted.
   It is the default and the tie-breaker when nobody has a device to test.
3. **Vendor XML from the corpus.** Real devices' own descriptions of
   themselves — weaker than the device but far stronger than intuition,
   and available without hardware.
4. **`../aravis` and Wireshark's `packet-gvcp.c`.** Useful corroboration,
   *not* an authority. aravis is a mature independent implementation, so
   agreeing with it is reassuring and disagreeing with it is worth a hard
   look — but it has its own bugs and approximations, and matching them
   is not a goal. Cite it as "aravis does X", never as "the correct
   behaviour is X".

Consequence for day-to-day work: **do not close a hardware-dependent
question by reasoning about aravis.** Say what the spec requires, say what
aravis does, implement the safer reading, and put the open question in
`docs/backlog.md` so it can be settled the next time a reporter with the
relevant device appears. TC-09 (the 0x0004/0x0005 `BYE` vs `FORCEIP`
disagreement) is the worked example.

This is also why the issue tracker matters more than it looks: a user
report is not a support burden, it is the highest-quality evidence the
project can obtain. See "Vendor XML corpus" above — ask for the XML, and
add it.

### Statements to reporters and contributors

The evidence hierarchy applies to **our own claims** as much as to wire
questions. Every public statement — issue replies, PR reviews, the
changelog, the backlog — must be grounded in evidence actually checked,
not inferred or remembered. In particular:

- No "first", "only", "confirmed" or "proven" without checking the
  tracker for counterexamples. Several contributors have tested this
  library on real hardware; an unearned superlative erases their work.
- Distinguish what a reporter observed from what we inferred from their
  artifacts, and say which is which.
- Verify concrete claims before publishing them: parse the attached
  capture, resolve the real comment URL, check crates.io before telling
  anyone to install a version.

**Reviews of external PRs** exist to land the contribution, not to
showcase the review. Be concise and focused: request changes only for
material problems — correctness, soundness, CI-blocking failures.
Style preferences and nice-to-haves are optional notes, or follow-ups
we do ourselves after the merge. For every mechanical change requested,
attach a one-click GitHub ```suggestion``` block (anchored to diff
lines) so the contributor's remaining work is only the parts that need
their judgment — or their hardware.

**A suggestion block must be code we have actually run through the
gates.** A one-click suggestion is a commit we are authoring in someone
else's branch, so it carries our obligations, and the contributor has no
reason to re-check it. On #72 two hand-written suggestions turned a
passing CI red: four struct literals were written on one line, which
rustfmt expands whenever the body exceeds `struct_lit_width` (18), and an
`#[allow(clippy::await_holding_lock)]` was placed on a `let` statement,
where it cannot work — that lint resolves its level at the enclosing
coroutine body, so the attribute has to sit on the function. Apply the
change locally, run `/quality-gate`, and paste the formatted result.

**When we break a contributor's CI, we fix it — on their branch.**
`gh pr merge` is not the only maintainer tool; `maintainerCanModify` lets
us push the repair directly, keeping their authorship and costing them no
round-trip. Do not ask a contributor to fix our whitespace.

### Fake camera binary

For interactive testing or E2E testing with Viva Studio:

```bash
# Start fake camera (stays alive until Ctrl+C)
cargo run -p viva-fake-gige
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15 --pixel-format rgb8

# Use CLI to interact
cargo run -p viva-camctl -- list --iface 127.0.0.1

# E2E with studio — GigE (3 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5
# T3: export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
#     cd studio/apps/viva-studio-tauri && cargo tauri dev

# E2E with studio — USB3 Vision fake camera (2 terminals)
# T1: cargo run -p viva-service-u3v -- --fake --zenoh-config studio/config/zenoh-local.json5
# T2: export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
#     cd studio/apps/viva-studio-tauri && cargo tauri dev
```

```bash
# Test FORCEIP with fake GigE camera (2 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --force --iface 127.0.0.1

# Test persistent IP with fake GigE camera (2 terminals)
# T1: cargo run -p viva-fake-gige
# T2: cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --iface 127.0.0.1
```

**Important**: both sides need their own Zenoh config, and neither is
automatic. The **service** takes `--zenoh-config studio/config/zenoh-local.json5`
(GigE and U3V alike). The **studio** reads the `ZENOH_CONFIG` environment
variable — there is no default path, no dev-mode auto-detection, and no
`--zenoh-config` flag on that binary. Point it at `zenoh-studio.json5` with an
absolute path, because the walkthrough `cd`s into the app directory first.

This file used to claim the studio "loads its own Zenoh config automatically in
dev mode". It never did. Without `ZENOH_CONFIG` the app starts in *embedded*
mode, whose GigE discovery deliberately skips loopback (`backend/embedded.rs`,
guarding against #57) — so the fake camera is not merely slow to appear, it can
never appear, and the device list stays empty with no error. That is #132. The
app now shows its mode in the header and reports an unloadable `ZENOH_CONFIG`
as an error instead of silently falling back.

## Documentation

- **mdBook**: `book/` directory - tutorials, architecture, networking cookbook (published user docs)
- **API docs**: Generated via `cargo doc`, published to GitHub Pages
- **Examples**: 18 examples in `crates/viva-genicam/examples/` (including `demo_fake_camera` for zero-hardware demo)

**The book's Rust snippets come from these examples**, via mdBook
`{{#include <file>:<anchor>}}` against `// ANCHOR:` / `// ANCHOR_END:`
comments. That is deliberate: `cargo clippy --workspace --all-targets`
compiles every example, so a snippet cannot drift from the API it
documents. Book chapters used to describe an API that never existed
(`viva_genicam::Client`, `viva_gige::control::ControlClient`,
`genicam::Context::new`) — DOC-01. When adding a snippet, anchor it in a
real example rather than hand-writing it, and never delete an anchor
without checking who includes it.

mdBook will not tell you when this breaks: a missing include *file* logs
`[ERROR]` and still exits 0, and a missing *anchor* renders as an empty
code block with no diagnostic at all. `crates/viva-genicam/tests/book_includes.rs`
is the real gate and runs under `cargo test --workspace`; `ci.yml`'s lint
job additionally greps `mdbook build` output for `[ERROR]`.
- **Standards intro**: `docs/standards.md` - what GenApi/GenCP/GVCP/SFNC/PFNC are and how they map to crates
- **Development docs**: `docs/` - see the next section

## Development Docs

- [docs/design.md](docs/design.md) — architecture, key abstractions, data flows, design tenets
- [docs/roadmap.md](docs/roadmap.md) — mid-term direction by phase
- [docs/backlog.md](docs/backlog.md) — immediate actionable tasks; pick up work from here
- [docs/adrs/](docs/adrs/) — decision records; add one via the template in `docs/adrs/README.md` when making an architectural decision (retrospective ADRs welcome)

## Skills

Project skills in `.claude/skills/` (invoke with `/quality-gate` etc.):

- **quality-gate** — run all CI gates locally before pushing (library workspace + studio when touched)
- **implement-task** — implement a backlog task from `docs/backlog.md` end-to-end (plan → subagent implement → review → PR)
- **adr-new** — scaffold the next ADR in `docs/adrs/` and index it
- **release** — cut a release: version bump touchpoints, changelog, PR, tags, publish verification

## Code Intelligence (codegraph)

A codegraph MCP index of the workspace lives in `.codegraph/` (gitignored, regenerable). Consult `codegraph_context` / `codegraph_trace` / `codegraph_search` BEFORE exploring by grep — it's sub-millisecond and already built. The file watcher keeps the index fresh (~1 s lag); if results look stale, check `codegraph_status`.

## Shared Crate API (SX handoff)

`viva-genapi-xml` and `viva-genapi` are designed for external consumption by Viva Studio (`studio/`):
- All `viva-genapi-xml` public types derive `Serialize`/`Deserialize` (serde)
- `viva-genapi` provides introspection: `NodeMap::node_names()`, `dependents()`, `categories()`, `Node::kind_name()`, `access_mode()`, `name()`
- `NullIo` enables offline XML browsing without a camera
- Both crates compile for `wasm32-unknown-unknown`
- `fetch_and_load_xml` is behind the `fetch` feature flag (default on)

## Version Bumps

A single release version is shared by all workspace crates plus the
Python package. When cutting a new version, update all six touch-
points together:

1. `Cargo.toml` — `[workspace.package] version` (picked up by every
   crate that uses `version.workspace = true`).
2. `crates/viva-pygenicam/Cargo.toml` — `[package] version` (this
   crate does not inherit from the workspace so it must be bumped
   explicitly).
3. `crates/viva-pygenicam/pyproject.toml` — `[project] version`
   (this is what PyPI reads).
4. `crates/viva-pygenicam/python/viva_genicam/__init__.py` —
   `__version__ = "X.Y.Z"` (what `viva_genicam.__version__` returns
   at runtime).
5. `CHANGELOG.md` — new `## [X.Y.Z] - YYYY-MM-DD` entry plus a link
   line in the footer. Follow Keep a Changelog categories (Added /
   Changed / Fixed / etc.).
6. **Intra-workspace dependency ranges** in every `crates/*/Cargo.toml`
   and `studio/**/Cargo.toml` — `viva-foo = { version = "0.2", path =
   ... }`. These are caret ranges, so a minor bump makes them
   unsatisfiable once published; they only keep building locally
   because `path` wins. Bump them whenever the minor version changes.

A missed file will either break the wheel build (mismatched crate
vs pyproject version) or publish with the wrong metadata.

**Do not add a version-pinned install snippet to any README.** Both
READMEs say `cargo add viva-genicam`, which always resolves to what is
actually published. The crate README used to carry
`viva-genicam = "X.Y"`; nothing builds that line, so it rotted to `"0.1"`
and stayed there through all of 0.2, and any tree-vs-crates.io value it
could hold is wrong at one end or the other — the tree carries the next
version while crates.io still serves the last one.

## Dependency Upgrades

Before bumping a crate's version in any `Cargo.toml`, always check crates.io for the latest
release, or the crate's crates.io page. Don't assume the version the user names is current;
verify it exists and whether a newer one is available before editing manifests.

**Send a `User-Agent`.** crates.io enforces its API data-access policy by
rejecting unidentified requests, and the failure is silent through `jq`:

```bash
# Returns the string "null" — an error body, not a missing version.
curl -sSL https://crates.io/api/v1/crates/<name> | jq -r .crate.max_version

# Correct.
curl -sSL -H 'User-Agent: viva-genicam-dev' \
  https://crates.io/api/v1/crates/<name> | jq -r .crate.max_version
```

The first form is what this file used to recommend. `null` reads as "not
published", so it can talk you out of a correct release or into
re-publishing one — check the raw body before believing an absence.

## Standards

This codebase implements these EMVA standards:
- **GenApi** - XML-based node description (Tier-1 + Tier-2 including pValue delegation)
- **GVCP/GVSP** - GigE Vision Control/Streaming Protocols
- **GenCP** - Generic Control Protocol
- **PFNC/SFNC** - Pixel Format and Standard Feature Naming Conventions
