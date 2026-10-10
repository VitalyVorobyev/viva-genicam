# ADR-0017: Studio Monorepo with Two Cargo Workspaces

**Status:** Accepted; its CI trigger is amended by [ADR-0024](adr0024-studio-reference-viewer.md)
**Date:** 2026-07-25

## Context

Viva Studio (the Tauri desktop app) lived in a separate repository,
genicam-studio, consuming the viva-* library crates from crates.io. The
split produced observable drift, not just theoretical risk:

- Studio was pinned to viva-* 0.2.3 while the library moved on to 0.2.6;
  every wire-contract change required a publish-then-bump round-trip that
  in practice never happened.
- Studio CI checked out a GitHub repository name that no longer exists
  (pre-rebrand), so its e2e pipeline had been silently broken for months.
- ADR-010 (FeatureState contract) was referenced from this repository's
  CHANGELOG but lived in the other repo — cross-repo doc links rotted.
- All real studio work was stranded on a feature branch
  (`12-genicam-ws-streamer`) that main never caught up with.

## Decision

One git repository, **two Cargo workspaces**.

The studio tree is imported under `studio/` as a squash commit (no history
merge); provenance — source repo, branch, and commit — is recorded in the
commit message, and the original repository will be archived. `studio/`
keeps its own `[workspace]`, its own `Cargo.lock` (plus the separate
lockfile of the Tauri app, which stays excluded from the studio workspace
and is built via `cargo tauri`), and depends on the library via **path
dependencies** into `crates/`.

The published library workspace at the repo root deliberately does not
absorb the studio crates: its dependency graph, cargo-deny scope, MSRV,
and release tags stay decoupled from GUI/Tauri/Node concerns. The root
workspace simply adds `studio` to `[workspace] exclude`.

## Consequences

**Positive:**

- Wire-contract changes (viva-zenoh-api, UiGraph) are atomic: one PR
  changes the type and every consumer, with no version lag and no
  crates.io round-trip.
- One docs tree and one backlog; studio ADRs 0001–0010 sit next to
  library ADRs 0011+ and links cannot rot across repos.
- The library release pipeline, supply-chain gate (deny.toml), and MSRV
  are untouched — publishing viva-* crates involves zero GUI code.

**Negative:**

- The repository carries a Node/bun toolchain and GUI assets; library
  contributors see (and clone) code they never build.
- Studio needs its own path-filtered CI workflow; a green root CI no
  longer implies the studio tree builds.
- Two lockfiles for one dependency graph slice means occasional duplicate
  lock churn when shared deps bump.

**Alternatives rejected:**

- *Single workspace*: couples release versioning, cargo-deny scope, and
  licensing review of the published crates to Tauri/GUI dependencies.
- *Staying in a separate repo*: the observed drift above — version lag,
  dead CI, stranded branches, rotting cross-repo references.
