# ADR-0024: Viva Studio is the library's reference viewer

**Status:** Accepted
**Date:** 2026-10-10

## Context

Viva Studio began as a side experiment: a separate repository imported into
`studio/` in July 2026 ([ADR-0017](adr0017-studio-monorepo-two-workspaces.md)).
It has not shipped a binary, and its version still reads `0.1.0`. People use it
anyway, and what they do with it has been more useful than anything else the
project receives:

- **Windows 11, JAI cameras, direct APIPA links.** Reporters built Studio
  from source and found that discovery failed and that acquisition had only ever
  worked against the loopback fake ([#57], [#70]). Studio also showed Mono8 and
  SingleFrame whatever the camera was set to ([#87]), and a Command node looked
  like plain text rather than a button ([#110]). On one link, Studio overwrote
  the packet size on every start and received 0 frames ([#112]). Three of these
  were fixed by the reporters' own PRs (#72, #87, #118).
- **Ubuntu 24.04, several identical cameras.** Another reporter hit the Zenoh
  configuration trap ([#132]), a bulk read of a write-only register ([#135]),
  and a device list that could not tell identical cameras apart ([#137]).

That is eight reports from three GitHub accounts, every one of them on a
Studio built from source. Studio is how hardware evidence reaches this project
(CLAUDE.md, *Evidence hierarchy*): it is the fastest way for someone who owns a
camera to exercise the library end to end.

A review of the code on 2026-10-10 found sound foundations but a half-finished
middle:

- **Sound foundations.** Frames travel as BMP over a localhost WebSocket into
  `createImageBitmap`, so no pixel goes through JSON or Tauri IPC. The UI is
  strict TypeScript on design tokens. `viva_xml_model` maps the library's
  `NodeMap` rather than parsing XML a second time.
- **The backend abstraction is half built.** `DeviceBackend` has one real
  implementation. `RemoteBackend` returns `Err` from every method, so a dozen
  Tauri commands branch on the mode instead. Remote mode is one contract version
  behind the service: it ignores the `FeatureState` topics `viva-service`
  publishes.
- **Two copies of the feature-state logic.** The embedded backend's
  `build_feature_state` is a stale copy of `viva-service`'s, and it hard-codes
  `is_available`/`is_implemented` to `true`.
- **The UI migration stopped halfway.** [ADR-0010](adr0010-feature-state-contract.md)
  made `FeatureState` the authoritative live state. Its step 5 — the UI
  consuming it — was started and not finished: the UI keeps two value caches,
  and the one meant to be authoritative is never seeded. The TypeScript types
  are hand-written and have already drifted from the Rust ones.
- **Coverage gaps.** CI compiles the Tauri app only on macOS, which no known user
  runs. A library-only PR never triggers Studio CI (#105 broke it; #106 repaired
  it). Nothing drives the backend against `viva-fake-gige`, and the UI tests
  cover pure functions only.

## Decision

1. **Role.** Studio is the library's **reference viewer**: a single-camera tool
   in the class of pylon Viewer, SpinView and aravis-viewer, for configuring,
   viewing and diagnosing a camera and for filing a report about it. Embedded
   mode — the app driving the camera through `viva-genicam` in-process — is the
   primary path. Multi-camera remains out of scope
   ([ADR-0004](adr0004-single-camera-scope.md)) until a user needs it.
2. **Remote mode stays, behind one trait.** `DeviceBackend` gets a second real
   implementation that consumes the service's `FeatureState` topics, and the
   per-mode branches in the Tauri commands go away. Remote mode is what the
   Zenoh services are for, and at least one reporter runs it ([#109]).
3. **One feature-state implementation, owned by the library.** The embedded
   backend and `viva-service` compute `FeatureState` with the same code. Where
   that code lives is settled by the PR that moves it; the constraint is that
   neither consumer keeps a copy. *Settled:* `viva-genapi` computes a typed
   snapshot (`NodeMap::feature_state`), and `viva-zenoh-api`'s `genapi`
   feature converts it to the wire form — the snapshot is GenApi's business,
   the JSON spelling the contract's.
4. **One contract.** The TypeScript types for IPC payloads are generated from
   the Rust types, and CI fails when the generated file is stale. *Settled:*
   ts-rs, behind a `ts` feature on `viva-zenoh-api` and `viva_xml_model`; the
   Tauri crate's `bindings.rs` test writes and checks
   `ui/viva-studio-ui/src/generated/ipc.ts`, including a map from each event
   name to its payload.
5. **Distribution.** Every release tag attaches **unsigned** desktop bundles —
   DMG, AppImage/deb and MSI — with documentation for the Gatekeeper and
   SmartScreen prompts. Studio carries the single workspace version
   ([ADR-0012](adr0012-layered-crate-architecture.md)), and the release skill
   bumps it with the rest. Signing is revisited if users ask for it.
6. **Testing layers.** In order of value:
   - The Tauri backend runs against `viva-fake-gige` in-process: connect, read
     feature state, write, receive frames, and release the camera on disconnect.
   - The generated-type check from point 4.
   - UI hook and component tests on jsdom with Testing Library and Tauri's
     `mockIPC`.
   - ESLint with the react-hooks rules.
   - Tauri compiled and tested on Linux, Windows and macOS.
   - Studio CI also triggers on the library paths it depends on.

   Browser end-to-end tests wait until the data layer has settled.
   `tauri-driver` is not adopted, because it does not support macOS.
7. **Phases.** The backlog's ST rows are ordered by these phases:
   - 0, stop the bleeding: the confirmed defects, the test and lint
     infrastructure, and CI coverage.
   - 1, foundations: points 2–4, the camera lifecycle, and dead-code removal.
   - 2, viewer parity: ordered by user evidence first.
   - 3, distribution: point 5. It can run alongside phase 2.
   - 4, later: recording and playback, annotations, auto-update, multi-camera.

## Consequences

**Positive:**

- Every hardware owner who can run Studio can also produce evidence. Phase 2's
  "generate report" action makes that one click.
- Fixing feature state once fixes it for Studio, the service and every client
  of the service.
- Users on Windows and Linux are covered by CI on the OS they actually run.

**Negative:**

- Studio becomes a maintained product with a release surface: bundles to build
  on three OSes, and a version to keep in step.
- Unsigned bundles make the first launch awkward on macOS and Windows.
- Generated types add a code-generation step to the Rust–TypeScript boundary.

**Amends** [ADR-0017](adr0017-studio-monorepo-two-workspaces.md): Studio CI
triggers on `crates/**` and the root `Cargo.toml` as well as `studio/**`, so a
green library PR implies a building Studio again.

[#57]: https://github.com/VitalyVorobyev/viva-genicam/issues/57
[#70]: https://github.com/VitalyVorobyev/viva-genicam/issues/70
[#87]: https://github.com/VitalyVorobyev/viva-genicam/pull/87
[#109]: https://github.com/VitalyVorobyev/viva-genicam/issues/109
[#110]: https://github.com/VitalyVorobyev/viva-genicam/issues/110
[#112]: https://github.com/VitalyVorobyev/viva-genicam/issues/112
[#132]: https://github.com/VitalyVorobyev/viva-genicam/issues/132
[#135]: https://github.com/VitalyVorobyev/viva-genicam/issues/135
[#137]: https://github.com/VitalyVorobyev/viva-genicam/issues/137
