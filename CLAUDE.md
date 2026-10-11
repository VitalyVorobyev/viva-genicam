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
cargo test --workspace                     # unit, integration (fake cameras), service e2e, doc checks
RUST_LOG=debug cargo test --workspace -- --nocapture

cargo run -p viva-service -- --iface en0   # GigE sensor service
cargo run -p viva-camctl -- list           # CLI
```

Each integration suite, and how to run it alone, is listed in
[CONTRIBUTING.md → Tests](CONTRIBUTING.md#tests). Every fake-camera suite binds
UDP 3956: two test processes running at once (e.g. in two worktrees) cross-talk
and fail spuriously.

## Pre-Push Checklist

Before pushing to a remote branch, always run these gates locally (the
**quality-gate** skill runs the full set, including the studio and
`viva-pygenicam` workspaces):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

CI rejects all three: its `cargo doc` runs with `--all-features` and
`RUSTDOCFLAGS=-D warnings`. CI also runs what this list does not: a
per-feature check (`cargo hack --each-feature`), a `cargo check` on the
declared MSRV, and a `wasm32-unknown-unknown` check of `viva-genapi-xml` and
`viva-genapi`.
Transitive feature unification can mask breakage locally (a workspace sibling
may already pull a crate in with extra features) — don't rely on "it built on
my machine".

## Architecture

The crate map and strict layering, key abstractions, data flows and **design
tenets** live in [docs/design.md](docs/design.md); when a change conflicts with
a tenet, write or update an ADR in `docs/adrs/` rather than silently deviating.

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
`cargo test --workspace`; the suites are listed in
[CONTRIBUTING.md](CONTRIBUTING.md#tests).

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
The `Vendor XML Corpus` workflow runs it weekly and on demand. Why the
documents are fetched, the no-op without them, and `VIVA_GENICAM_XML_CORPUS`
are in [CONTRIBUTING.md](CONTRIBUTING.md#vendor-xml-corpus).

**When a user reports a camera we cannot open, ask for their XML and add it to
the corpus** — that is how this class of bug stops recurring. Point them at the
command, not a code snippet. Both work on a camera we cannot open: `xml` never
parses the document, and `report` records a failed nodemap build in the bundle
instead of stopping:

```bash
viva-camctl report --ip <CAMERA-IP> --out viva-report.txt   # everything
viva-camctl xml    --ip <CAMERA-IP> --out camera.xml        # just the XML
viva-camctl report --usb 0 --out viva-report.txt            # a USB3 Vision camera
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

### Viva Studio against a fake camera

Studio's remote mode needs a Zenoh config on **both** sides, and neither is
automatic: the service takes `--zenoh-config studio/config/zenoh-local.json5`
(GigE and U3V alike); Studio reads only the `ZENOH_CONFIG` environment variable
— no default path, no dev-mode auto-detection, no flag — which must be an
absolute path to `studio/config/zenoh-studio.json5`. Without it Studio runs in
*embedded* mode, whose GigE discovery deliberately skips loopback
(`backend/embedded.rs`), so the fake camera never appears. The recipes are in
[docs/studio/testing-cookbook.md](docs/studio/testing-cookbook.md).

## Documentation

User and developer documentation are kept apart:

- **mdBook** (`book/`) — the published **user guide**: getting started,
  tutorials, networking, Python, Viva Studio, troubleshooting. Nothing about
  working on the code goes here; the book's last page points developers to
  the repository.
- **`CONTRIBUTING.md`** and **`docs/`** (indexed by `docs/README.md`) — the
  **developer** documentation: building, checks, test suites, design, ADRs,
  roadmap, backlog. `studio/README.md` is Studio's developer notes.
- **API docs** — `cargo doc`, published to GitHub Pages.
- **Examples** — `crates/viva-genicam/examples/` (`demo_fake_camera` is the
  zero-hardware demo).
- **Standards** — [docs/standards.md](docs/standards.md): which crate
  implements which EMVA standard — GenApi (Tier-1 + Tier-2, including pValue
  delegation), GenCP, GVCP/GVSP, USB3 Vision, SFNC, PFNC.
- **Changelog** — `CHANGELOG.md` is an index plus `[Unreleased]`; each release
  series has its own file in `changelog/`. Entries follow
  [CONTRIBUTING's changelog rules](CONTRIBUTING.md#the-changelog).
- **Known gaps** — [issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191)
  is the public list of what does not work yet; the book's welcome page and the
  READMEs link to it instead of listing gaps. When a fix for one of its items
  merges, tick that item on #191.

**One fact, one place.** Each fact has one canonical page per audience
(CONTRIBUTING's Documentation section names them); everywhere else links to it
rather than repeating it.

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

[docs/README.md](docs/README.md) indexes them. Pick up work from
[docs/backlog.md](docs/backlog.md); record an architectural decision as an ADR
in `docs/adrs/` (template in `docs/adrs/README.md`, or `/adr-new`;
retrospective ADRs welcome).

## Skills

Project skills in `.claude/skills/` (invoke with `/quality-gate` etc.):

- **quality-gate** — run all CI gates locally before pushing (library workspace + studio when touched)
- **implement-task** — implement a backlog task from `docs/backlog.md` end-to-end (plan → subagent implement → review → PR)
- **adr-new** — scaffold the next ADR in `docs/adrs/` and index it
- **release** — cut a release: version bump touchpoints, changelog, PR, tags, publish verification

## Code Intelligence (codegraph)

A codegraph MCP index of the workspace lives in `.codegraph/` (gitignored, regenerable). Consult `codegraph_context` / `codegraph_trace` / `codegraph_search` BEFORE exploring by grep — it's sub-millisecond. The file watcher keeps the index fresh (~1 s lag); if results look stale, check `codegraph_status`. If the MCP server is unavailable, fall back to grep or an Explore subagent.

## Version Bumps

All workspace crates, the Python package and Viva Studio share one release version, bumped
at eight touchpoints together — the **release** skill
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
