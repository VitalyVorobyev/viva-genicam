---
name: release
description: Cut a release: version bump across all touchpoints, changelog, PR, tags, publish verification
---

# Release

One version is shared by all workspace crates plus the Python package; this
skill is the canonical list of where it lives. `studio/` crates are unpublished
and not versioned by this process.

## 1. Bump — all six touchpoints together

1. `Cargo.toml` — `[workspace.package] version` (every crate with
   `version.workspace = true` picks it up)
2. `crates/viva-pygenicam/Cargo.toml` — `[package] version` (does not
   inherit from the workspace)
3. `crates/viva-pygenicam/pyproject.toml` — `[project] version` (what PyPI
   reads)
4. `crates/viva-pygenicam/python/viva_genicam/__init__.py` — `__version__`
5. `CHANGELOG.md` — rename `[Unreleased]` → `## [X.Y.Z] - YYYY-MM-DD`,
   add a fresh empty `[Unreleased]` section, add the footer link line
   (Keep a Changelog categories: Added / Changed / Fixed / …)
6. **Intra-workspace dependency ranges** in every `crates/*/Cargo.toml`
   and `studio/**/Cargo.toml` — `viva-foo = { version = "0.2", path = ... }`.
   These are caret ranges, so a minor bump makes them unsatisfiable once
   published; they only keep building locally because `path` wins. Bump
   them whenever the minor version changes.

A missed file breaks the wheel build or publishes wrong metadata.

Do **not** add a version-pinned install snippet to a README — both use
`cargo add viva-genicam` precisely so there is no seventh thing to forget.
See "Version Bumps" in `CLAUDE.md`.

## 2. Verify locally

- Refresh **all four** lockfiles — `cargo metadata --format-version 1 > /dev/null`
  run from each of:

  ```
  .                                        # library workspace
  crates/viva-pygenicam                    # its own workspace, not a member
  studio                                   # second workspace (ADR-0017)
  studio/apps/viva-studio-tauri/src-tauri  # third workspace
  ```

  Every crate version appears in each of them, so a stale one fails CI rather
  than merely looking untidy: the `lint viva-pygenicam` job runs
  `cargo clippy --locked`, and `--locked` refuses to update the file. Missing
  `crates/viva-pygenicam/Cargo.lock` is what broke the 0.4.1 release PR — it is
  easy to forget precisely because that crate is also the one manifest that
  does not inherit `version.workspace`.
- Run the **quality-gate** skill — all gates must pass.

## 3. Release PR

Feature branch, commit, PR titled `Release X.Y.Z`. Never push to main.

## 4. After merge: tag

```bash
git tag vX.Y.Z <merge-commit>
git tag py-vX.Y.Z <merge-commit>
git push origin vX.Y.Z py-vX.Y.Z
```

## 5. Watch and verify publication

Watch the three workflows: **Publish Rust crates**, **Release**,
**Python wheels** (`gh run list`/`gh run watch`). Then verify:

- crates.io sparse index shows the new version:
  `curl -A viva-release-check https://index.crates.io/vi/va/viva-genicam | tail -1`
- PyPI JSON shows the version and license `MIT AND LGPL-2.1-or-later`:
  `curl -s https://pypi.org/pypi/viva-genicam/json | jq '.info.version, .info.license'`
- GitHub release `vX.Y.Z` exists and has the binary assets attached.

Report a checklist of what was verified and anything still pending.
