# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

viva-genicam is a pure Rust implementation of GenICam ecosystem building blocks supporting GigE Vision and USB3 Vision. It provides libraries and CLI tools for camera discovery, control, streaming, and feature access.

We do not maintain backward compatibility at this early development stage. The priority is clear design and structure.

## Related Projects

- **Viva Studio** (`studio/`) — Tauri desktop app, second Cargo workspace in this repo (see `studio/CLAUDE.md`).
- **aravis** (`../aravis`) — C library for GenICam cameras. Optional; a corroborating second opinion when reading the wire protocols, not an authority on them (see [Evidence hierarchy](#evidence-hierarchy)). Not required for development or CI.

## Commands

```bash
cargo build --workspace
cargo test --workspace                     # unit + integration + service e2e
RUST_LOG=debug cargo test --workspace -- --nocapture

# Integration suites (in-process fakes; no hardware, no external tools)
cargo test -p viva-genicam --test fake_camera        # 38: GigE discovery, features, streaming
cargo test -p viva-genicam --test predicates         # 6: pIsLocked, pIsAvailable, ... on the fake camera
cargo test -p viva-genicam --test heartbeat          # 2: idle keepalive survival + a negative control
cargo test -p viva-genicam --test register_access    # 6: READREG/WRITEREG dispatch and fallback
cargo test -p viva-genicam --test fake_u3v_camera --features u3v  # 5: U3V open, features, streaming, pixel formats
cargo test -p viva-service --test fake_camera_e2e    # 6: acquisition, double start, sustained streaming,
                                                     #    predicates, write-only/command snapshot, EVS topic
cargo test -p viva-genicam --test book_includes      # 1: every mdBook {{#include}} resolves

# Every fake-camera suite binds UDP 3956: two test processes running at once
# (e.g. in two worktrees) cross-talk and fail spuriously.

cargo run -p viva-service -- --iface en0   # GigE sensor service
cargo run -p viva-camctl -- list           # CLI
```

## Pre-Push Checklist

Before pushing to a remote branch, always run these gates locally (the
**quality-gate** skill runs the full set, including the studio and
`viva-pygenicam` workspaces):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

CI rejects fmt and clippy failures; its `cargo doc` step does not yet deny
warnings (backlog CI-08), so the local doc gate is the one that catches them.
Transitive feature unification can mask breakage locally (a workspace sibling
may already pull a crate in with extra features) — don't rely on "it built on
my machine".

## Architecture

Strict layering, bottom to top: `viva-gencp` → `viva-gige` / `viva-u3v` →
`viva-genapi-xml` → `viva-genapi` → `viva-genicam` (facade) → `viva-service` /
`viva-service-u3v` (Zenoh bridges), with `viva-camctl` and `viva-pygenicam` on
top. The crate map, key abstractions, data flows and **design tenets** live in
[docs/design.md](docs/design.md); when a change conflicts with a tenet, write or
update an ADR in `docs/adrs/` rather than silently deviating.

`viva-genapi-xml` and `viva-genapi` are consumed by Viva Studio (`studio/`), so
keep these constraints:

- Every `viva-genapi-xml` public type derives `Serialize`/`Deserialize`.
- Both crates compile for `wasm32-unknown-unknown`.
- `NullIo` enables offline XML browsing without a camera; `viva-genapi`'s
  introspection (`NodeMap::node_names()`, `dependents()`, `categories()`,
  `Node::kind_name()`, `access_mode()`, `name()`) serves the studio.
- `fetch_and_load_xml` is behind the `fetch` feature (default on).

## Testing

Unit tests live in source modules (`mod tests { }`). Integration tests use the
in-process fakes `viva-fake-gige` and `viva-fake-u3v` and run under
`cargo test --workspace`; the suites are listed under [Commands](#commands).

### Vendor XML corpus

Fake cameras only exercise constructs we already thought of; real vendor GenApi
XML is where the surprises live. The corpus is real device descriptions fetched
from third-party projects and from issue attachments, plus three documents that
are not vendor hardware (the GenICam conformance document and two aravis
synthetic devices). Provenance for each file is in
`scripts/fetch-xml-corpus.sh`; count them with
`ls fixtures/vendor-xml/*.xml | wc -l` rather than quoting a number.

```bash
scripts/fetch-xml-corpus.sh        # into fixtures/vendor-xml/ (gitignored)
cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture  # parses
cargo test -p viva-genapi     --test vendor_corpus -- --nocapture  # + evaluates
```

The `viva-genapi` stage builds a `NodeMap` from each document and evaluates
every node against a stub transport — that is where formulas, addressing and
numeric codecs are exercised, and where the #35 defects lived, invisible to the
XML-layer test ([ADR-0018](docs/adrs/adr0018-genapi-conformance-over-convenience.md)).
The documents are vendor copyright, so we fetch rather than redistribute them.
The test is a no-op when the directory is absent; the `Vendor XML Corpus`
workflow runs it weekly and on demand. Point `VIVA_GENICAM_XML_CORPUS` at
another directory to check XML dumped from your own hardware.

**When a user reports a camera we cannot open, ask for their XML and add it to
the corpus** — that is how this class of bug stops recurring. Point them at the
command, not a code snippet. Both work on a camera we cannot open: `xml` never
parses the document, and `report` records a failed nodemap build in the bundle
instead of stopping:

```bash
viva-camctl report --ip <CAMERA-IP> --out viva-report.txt   # everything
viva-camctl xml    --ip <CAMERA-IP> --out camera.xml        # just the XML
```

A node we cannot handle does not fail the document: the XML layer records it in
`XmlModel::skipped`, the GenApi layer in `NodeMap::skipped()`. Both are logged,
and both corpus tests fail on any skipped node not allowed by `EXPECTED_SKIPS`
(by document and node) or `EXPECTED_SKIP_REASONS` (by tag and error substring),
so new gaps surface instead of hiding.

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
relevant device appears. The worked example is opcode 0x0004/0x0005 — `BYE` in
aravis, `FORCEIP` in Wireshark — held open in
[ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md)
until a JAI FS-3200T capture on
[#57](https://github.com/VitalyVorobyev/viva-genicam/issues/57#issuecomment-5124465564)
settled it as `FORCEIP`.

A user report is not a support burden; it is the highest-quality evidence the
project can obtain — ask for the XML, and add it.

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

**A suggestion block must be code we ran through `/quality-gate`.** It is a
commit we author on someone else's branch, and the contributor has no reason to
re-check it. Apply the change locally, run the gates, and paste the formatted
result. Two pitfalls that hand-written suggestions have hit:

- rustfmt expands a struct literal onto multiple lines once its body exceeds
  `struct_lit_width` (18), so a one-line literal fails `cargo fmt --check`.
- `#[allow(clippy::await_holding_lock)]` on a `let` statement has no effect;
  the lint resolves its level at the enclosing coroutine body, so put it on the
  function.

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

# FORCEIP / persistent IP against the fake (T1: cargo run -p viva-fake-gige)
cargo run -p viva-camctl -- --iface 127.0.0.1 set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --force
cargo run -p viva-camctl -- --iface 127.0.0.1 set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100
```

**Important**: both sides need their own Zenoh config, and neither is
automatic. The **service** takes `--zenoh-config studio/config/zenoh-local.json5`
(GigE and U3V alike). The **studio** reads the `ZENOH_CONFIG` environment
variable — there is no default path, no dev-mode auto-detection, and no
`--zenoh-config` flag on that binary. Point it at `zenoh-studio.json5` with an
absolute path, because the walkthrough `cd`s into the app directory first.
Without `ZENOH_CONFIG` the studio runs in *embedded* mode, whose GigE discovery
deliberately skips loopback (`backend/embedded.rs`), so the fake camera never
appears.

## Documentation

- **mdBook** (`book/`) — published user docs: tutorials, architecture,
  networking cookbook.
- **API docs** — `cargo doc`, published to GitHub Pages.
- **Examples** — 18 in `crates/viva-genicam/examples/` (`demo_fake_camera` is
  the zero-hardware demo).
- **Standards intro** — [docs/standards.md](docs/standards.md): what
  GenApi/GenCP/GVCP/GVSP/SFNC/PFNC are and how they map to crates.

**The book's Rust snippets come from the examples**, via mdBook
`{{#include <file>:<anchor>}}` against `// ANCHOR:` / `// ANCHOR_END:`
comments, because `cargo clippy --workspace --all-targets` compiles every
example and so a snippet cannot drift from the API it documents. When adding a
snippet, anchor it in a real example rather than hand-writing it, and never
delete an anchor without checking who includes it.

mdBook will not tell you when this breaks: a missing include *file* logs
`[ERROR]` and still exits 0, and a missing *anchor* renders as an empty code
block with no diagnostic at all. `crates/viva-genicam/tests/book_includes.rs`
is the real gate and runs under `cargo test --workspace`; `ci.yml`'s lint job
additionally greps `mdbook build` output for `[ERROR]`.

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

A codegraph MCP index of the workspace lives in `.codegraph/` (gitignored, regenerable). Consult `codegraph_context` / `codegraph_trace` / `codegraph_search` BEFORE exploring by grep — it's sub-millisecond. The file watcher keeps the index fresh (~1 s lag); if results look stale, check `codegraph_status`. If the MCP server is unavailable, fall back to grep or an Explore subagent.

## Version Bumps

All workspace crates and the Python package share one release version, bumped
at six touchpoints together — the **release** skill
(`.claude/skills/release/SKILL.md`) lists them.

**Do not add a version-pinned install snippet to any README**; both say
`cargo add viva-genicam`, because nothing keeps a pinned line in step with what
crates.io serves.

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

`null` reads as "not published", so it can talk you out of a correct release or
into re-publishing one — check the raw body before believing an absence.

## Standards

The EMVA standards implemented here — GenApi (Tier-1 + Tier-2, including
pValue delegation), GVCP/GVSP, GenCP, PFNC/SFNC — and how they map to crates:
[docs/standards.md](docs/standards.md).
