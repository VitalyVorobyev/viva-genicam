---
name: release
description: Cut a release: version bump across all touchpoints, changelog, PR, tags, publish verification
---

# Release

One version is shared by all workspace crates, the Python package and Viva
Studio; this skill is the canonical list of where it lives. Studio's crates are
unpublished, but its desktop bundles are attached to the GitHub release under
the same version (ADR-0024).

## 1. Bump — all eight touchpoints together

1. `Cargo.toml` — `[workspace.package] version` (every crate with
   `version.workspace = true` picks it up)
2. `crates/viva-pygenicam/Cargo.toml` — `[package] version` (does not
   inherit from the workspace)
3. `crates/viva-pygenicam/pyproject.toml` — `[project] version` (what PyPI
   reads)
4. `crates/viva-pygenicam/python/viva_genicam/__init__.py` — `__version__`
5. The changelog. `CHANGELOG.md` is only an index plus `[Unreleased]`; each
   release series has its own file, `changelog/X.Y.md`.
   - Move the `[Unreleased]` entries into `changelog/X.Y.md` under
     `## [X.Y.Z] - YYYY-MM-DD`, newest release first. A new minor starts a new
     file with the same two-line header the others have; a patch goes on top
     of its series file.
   - Open the release with a short "Read before upgrading" list, then the Keep
     a Changelog categories (Added / Changed / Fixed / Removed). Edit the
     accumulated entries down to one bullet per user-visible change — what
     changed and why, with the PR link (the style rule in `CONTRIBUTING.md`).
   - Add `[X.Y.Z]: …/releases/tag/vX.Y.Z` at the bottom of the series file,
     add or extend the series row in the index table, leave `[Unreleased]`
     empty, and move its compare link to `vX.Y.Z...HEAD`.
   - `release.yml` copies `changelog/X.Y.md` into every CLI archive and fails
     the build if it is missing.
6. **Intra-workspace dependency ranges** in every `crates/*/Cargo.toml`
   and `studio/**/Cargo.toml` — `viva-foo = { version = "0.2", path = ... }`.
   These are caret ranges, so a minor bump makes them unsatisfiable once
   published; they only keep building locally because `path` wins. Bump
   them whenever the minor version changes.
7. `studio/Cargo.toml` — `[workspace.package] version` (Studio's crates
   inherit it)
8. `studio/apps/viva-studio-tauri/src-tauri/Cargo.toml` — `[package] version`.
   The Tauri app is a workspace of its own, and `tauri.conf.json` names no
   version, so the bundles take this one. The crate's `tests/version.rs`
   fails while 7 or 8 disagrees with 1.

A missed file breaks the wheel build or publishes wrong metadata.

Do **not** add a version-pinned install snippet to a README — both use
`cargo add viva-genicam` precisely so there is no seventh thing to forget.
See "Version Bumps" in `CLAUDE.md`.

## 2. Verify locally

- Refresh **all five** lockfiles — `cargo metadata --format-version 1 > /dev/null`
  run from each of:

  ```
  .                                        # library workspace
  crates/viva-pygenicam                    # its own workspace, not a member
  studio                                   # second workspace (ADR-0017)
  studio/apps/viva-studio-tauri/src-tauri  # third workspace
  fuzz                                     # the fuzz targets' own workspace
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
- PyPI JSON shows the version and license `MIT`:
  `curl -s https://pypi.org/pypi/viva-genicam/json | jq '.info.version, .info.license'`
- GitHub release `vX.Y.Z` exists and has the binary assets attached: the CLI
  archives; the GenTL producer for each of the three targets,
  `viva_gentl-x86_64-unknown-linux-gnu.cti`, `viva_gentl-aarch64-apple-darwin.cti`
  and `viva_gentl-x86_64-pc-windows-msvc.cti` (ADR-0031); the Viva Studio
  `.dmg`, `.AppImage`, `.deb` and `.msi`; and `SHA256SUMS.txt`, which covers
  them all. The **Release** workflow builds each `.cti` beside its target's
  archive (`scripts/build-cti.sh`) and uploads `dist/*.cti`.

Report a checklist of what was verified and anything still pending.
