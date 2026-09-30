---
name: quality-gate
description: Run all CI gates locally before pushing (library workspace + studio when touched)
---

# Quality Gate

Run the gates **in order** from the repo root. Stop at the first failure,
report it with the fix hint, and do not continue until it is fixed.

## Library workspace (always)

| # | Command | On failure |
|---|---------|------------|
| 1 | `cargo fmt --all --check` | run `cargo fmt --all`, then re-check |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | fix every warning — CI treats them as errors |
| 3 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | fix doc warnings (broken links, missing docs) |
| 4 | `cargo test --workspace` | show failing test names + output, fix, re-run |

## Studio workspace (only if files under `studio/` changed)

Check with `git diff --name-only main` (or staged changes). If anything
under `studio/` changed, additionally run in `studio/`:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`

And in `studio/ui/viva-studio-ui`:

4. `bun install --frozen-lockfile && bun run test -- --run && bun run build`

## The Tauri app is a *third* workspace (run it whenever a public type changes)

**`studio/apps/viva-studio-tauri/src-tauri` is not a member of the `studio/`
workspace.** `cargo clippy --workspace` in `studio/` does not reach it, and the
only thing that covers it is CI's `tauri-lint` job. Run:

```bash
cd studio/apps/viva-studio-tauri/src-tauri && cargo clippy --all-targets -- -D warnings
```

Run it whenever a public `enum`, trait or signature changes in the library
workspace, not only when `studio/` files are edited — a new public enum variant
can break this crate with nothing else local noticing.

## `crates/viva-pygenicam` is a FOURTH workspace — lint it separately

It is `exclude`d from the root workspace (`Cargo.toml`), so
`cargo clippy --workspace` never reaches it, and CI lints it in its own
`lint viva-pygenicam` job. Run:

```bash
cd crates/viva-pygenicam && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Run it on every toolchain update too: a new clippy lint can turn it red while
every other local gate stays green.

## The Python bindings are not covered by `cargo test` (run them when the pyo3
## surface changes)

`cargo test --workspace` does not build a wheel or run `pytest`, so a change to
a `#[pyo3]` signature is only checked by the `Python wheels` CI workflow. There
is no compile-time link between the pyo3 function and the hand-written wrapper
in `crates/viva-pygenicam/python/viva_genicam/`, which calls it **positionally**.

A renamed or reordered argument that is not updated in `camera.py`,
`_native.pyi`, the tests and the examples is a silent misconfiguration, not a
`TypeError` (Python's `bool` subclasses `int`). When touching the pyo3 surface,
grep the whole crate plus `book/src/python/` for the old name before pushing.

## Notes

- cargo-deny runs in CI (Linux). Run `cargo deny check` locally only if
  `deny.toml` or dependencies changed.
- Transitive feature unification can mask breakage locally — see
  "Pre-Push Checklist" in CLAUDE.md.

## Report

End with a compact table, one row per gate that ran:

```
| Gate                        | Result |
|-----------------------------|--------|
| cargo fmt (root)            | PASS   |
| cargo clippy (root)         | PASS   |
| cargo doc (root)            | PASS   |
| cargo test (root)           | PASS   |
| studio gates                | SKIP (no studio changes) |
```

If all pass: "All quality gates passed." Otherwise list actionable fixes.
