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
| 4 | `cargo check --locked --target wasm32-unknown-unknown -p viva-genapi-xml -p viva-genapi -p viva-pfnc` and `cargo check --locked --target wasm32-unknown-unknown -p viva-genapi-xml --no-default-features` | Studio browses XML in the browser through the first two crates, and `viva-pfnc`'s pixel conversion promises the same. The usual cause is a new dependency that does not build for wasm32 (`socket2`, tokio's `net`, anything compiling C): keep it out of them, or behind a feature that is off by default. Needs `rustup target add wasm32-unknown-unknown` once |
| 5 | `cargo test --workspace` | show failing test names + output, fix, re-run |

CI also runs two jobs that are too slow for every push; run them locally when
a crate's `[features]` or a dependency changes:

- **feature matrix** — `cargo hack check --workspace --each-feature --no-dev-deps`
  with `RUSTFLAGS="-D warnings"` (`cargo install cargo-hack`). Each crate alone,
  each feature alone: the workspace build unifies features and hides a crate
  that does not compile with its own defaults. `--no-dev-deps` rewrites the
  manifests while it runs, so run it in a copy of the tree, not in a checkout
  other work is happening in.
- **MSRV** — `cargo +<rust-version> check --workspace --locked`, with the
  version from the root `Cargo.toml` (`rustup toolchain install <version>`).

## Studio workspace (only if files under `studio/` changed)

Check with `git diff --name-only main` (or staged changes). If anything
under `studio/` changed, additionally run in `studio/`:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`

And in `studio/ui/viva-studio-ui`:

4. `bun install --frozen-lockfile && bunx tsc --noEmit && bun run lint && bun run test -- --run && bun run build`

   `bun run build` is Vite, which strips types without checking them, so
   `tsc --noEmit` is the only type check. Component tests (`*.test.tsx`) run
   on jsdom, which needs Node 22.22.2 or later on `PATH`.

## The Tauri app is a *third* workspace (run it whenever a public type changes)

**`studio/apps/viva-studio-tauri/src-tauri` is not a member of the `studio/`
workspace.** `cargo clippy --workspace` in `studio/` does not reach it, and the
only thing that covers it is CI's `tauri-lint` job. Run what it runs:

```bash
cd studio/apps/viva-studio-tauri/src-tauri
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Its backend tests drive both modes against `viva-fake-gige` on UDP 3956 (the
remote ones through `viva-service` in-process), so do not run them alongside
another fake-camera suite.

`cargo test` there also fails when `ui/viva-studio-ui/src/generated/ipc.ts` is
stale. Regenerate it with `UPDATE_BINDINGS=1 cargo test bindings` and commit
it — after any change to a type the UI receives, including the `viva-zenoh-api`
payloads and `viva_xml_model`'s UiGraph.

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

## The Python package: pytest and stubtest (whenever `crates/viva-pygenicam` or a type it wraps changes)

`cargo test --workspace` does not build a wheel or run `pytest`, so a change to
a `#[pyo3]` signature is otherwise only checked by the `Python wheels` CI
workflow. Build the module into a virtualenv and run both gates, the way
CONTRIBUTING describes (uv shown):

```bash
uv venv .venv && uv pip install --python .venv/bin/python maturin numpy pytest mypy
uv run --python .venv/bin/python maturin develop -m crates/viva-pygenicam/Cargo.toml
.venv/bin/python -m pytest crates/viva-pygenicam/tests
.venv/bin/python -m mypy.stubtest viva_genicam --allowlist crates/viva-pygenicam/stubtest-allowlist.txt
```

| Gate | On failure |
|------|------------|
| pytest | its suites start fake cameras on UDP 3956, so run it alone; `test_book_snippets.py` failing means a `<!-- pytest: … -->` snippet in `book/src/python/` drifted from the API |
| stubtest | `_native.pyi` disagrees with the built module, or the package's own code with either: update the stub, or list a deliberate gap in `stubtest-allowlist.txt` with the reason. Run it from the repository root, not from inside `crates/viva-pygenicam`, so `viva_genicam` resolves to the installed package |

There is no compile-time link between the pyo3 function and the hand-written
wrapper in `crates/viva-pygenicam/python/viva_genicam/`, which calls it
**positionally**. A renamed or reordered argument that is not updated in
`camera.py`, `_native.pyi`, the tests and the examples is a silent
misconfiguration, not a `TypeError` (Python's `bool` subclasses `int`). When
touching the pyo3 surface, grep the whole crate plus `book/src/python/` for the
old name before pushing.

## The GenTL producer: the consumer harness (whenever `crates/viva-gentl` or `viva-fake-gige` changes)

`cargo test -p viva-gentl` calls the producer's C functions in process; only
`scripts/gentl-harness.sh` loads the built `.cti` the way a consumer does. It
builds `viva_gentl.cti`, compiles a C program against the EMVA's `GenTL.h`,
acquires frames from `viva-fake-gige`, validates every port's XML against the
GenApi schema with `xmllint`, and, with `HARVESTERS_PYTHON` set, acquires again
through Harvesters. CI's `GenTL producer` job runs all of it on Linux.

```bash
HARVESTERS_PYTHON=/path/to/venv/bin/python scripts/gentl-harness.sh   # harvesters==1.4.3, genicam==1.6.0, numpy
```

It binds UDP 3956, so run it alone. Locally a step whose tool is missing is
skipped and says so; read the output rather than the exit code, and say in the
report which steps ran.

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
| wasm32 check                | PASS   |
| cargo test (root)           | PASS   |
| studio gates                | SKIP (no studio changes) |
| pytest + stubtest           | SKIP (pyo3 surface unchanged) |
| GenTL harness               | SKIP (viva-gentl and the fake unchanged) |
```

If all pass: "All quality gates passed." Otherwise list actionable fixes.
