# Roadmap

Mid-term direction, ordered by phase. **This file only looks forward**: shipped
work lives in [CHANGELOG.md](../CHANGELOG.md), actionable tasks and their
evidence in [backlog.md](backlog.md). Delete a phase when it closes.

**Ordering principle.** Priority is argued from measurement — a corpus count or
a user report — not intuition
([ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)). Phases are
numbered by when they were opened, not when they will close; the
[evidence hierarchy](../CLAUDE.md#evidence-hierarchy) routinely promotes work
from a later phase when a user with hardware appears. Corpus counts live only in
the backlog, so they are not repeated here.

## Phase 1 — Transport conformance (ADR-0019)

**Goal**: GVCP/GVSP implemented to the specification, with fake-camera wire
fixtures derived from the spec as byte arrays and asserted independently of the
client parser. Fake and client have repeatedly shared the same wrong assumption,
and only spec-derived fixtures stop them agreeing on an error
([design.md](design.md#testing-strategy)).

**Open**: TC-05, TC-06, TC-10, TC-11, TC-12, TC-16, TC-21, TC-24, TC-25, TC-26.

## Phase 2 — Diagnostics loop

**Goal**: every user can produce the artifact that diagnoses their camera, on a
camera we cannot open. Almost every fix so far came from a user-supplied
artifact, so producing one must be easy even when nothing is known to be wrong.

**Open**: none. Done: the report in every tool (ST-41), dropped features
named (DX-05), the report for USB3 Vision (DX-06).

## Phase 3 — Streaming reliability

**Goal**: the features that make the library trustworthy on a factory floor —
source filtering, packet resend (or its removal), telemetry that means what it
says, clean multicast teardown, an allocation-free receive path.

**Open**: SR-03, SR-04, SR-07, SR-08, SR-09, SR-12.

## Phase 4 — GenApi conformance, round 2

**Goal**: the GenApi constructs ADR-0018 did not reach, ordered by corpus
frequency — caching and polling, `pSelected` direction, dynamic limits, access
ceilings, `<pLength>`, a chunk adapter — and a corpus test that can fail on a
wrong *value*, not only on an error.

**Open**: GA-03, GA-04, GA-05, GA-07, GA-09, GA-10, GA-11, GA-12, GA-13, GA-14,
GA-15, GA-16, GA-17, GA-18, GA-23, GA-26, GA-30, GA-33.

## Not a phase — device classes beyond area-scan

Every assumption in this codebase is an area-scan camera's. A laser profile
scanner is in the corpus and its `Coord3D_*` formats are in `viva-pfnc`, but
nothing above that layer understands the frames; event cameras stream through
the reassembly path and are now published on their own `evs` topic. Only defects
verified against code or a wire capture are scheduled.

**Open**: DC-02, DC-03, DC-04, DC-05, DC-06.

## Phase 5 — API consolidation (breaking)

**Goal**: one deliberate breaking release that pays down surface-area debt —
typed and type-gated `Camera` methods, one frame-reassembly path, curated
re-exports, error source chains, a service `StreamSource` trait, `viva-pfnc` as
the single pixel-format authority. It lands in whichever breaking release is not
already claimed by a user's camera.

**Open**: TC-16, API-01, API-02, API-03, API-04, API-05, API-06, API-07, API-08,
API-09, API-10, API-11, API-13.

## Phase 6 — Production infrastructure

**Goal**: CI that verifies what we ship — per-crate feature sets, MSRV, the
Windows wheel, fuzzed parsers, semver checks — and documentation that is right,
not merely present.

**Open**: CI-01, CI-03, CI-04, CI-05, CI-07, CI-08, CI-09, CI-10, CI-17, DOC-06,
DOC-11; a book production-tuning chapter (rmem, udev, usbfs) is not yet a row.

## Services & Studio

**Goal**: the services behave like a product, and Studio becomes the library's
reference viewer ([ADR-0024](adrs/adr0024-studio-reference-viewer.md)) — the
fastest way for someone with a camera to exercise the library and report what
they saw.

- **Services**: SVC-01, SVC-03, SVC-04, SVC-05, SVC-06.
- **Studio foundations**: ST-45, ST-46. Done: one
  feature-state implementation (ST-26), one backend trait with a real remote
  implementation (ST-27), generated IPC types (ST-28), one UI data layer and
  `<FeatureControl>` (ST-29), re-reading what a write invalidates (ST-30),
  camera I/O off the runtime workers (ST-31), one frame encoder (ST-32), the
  React Compiler lint rules (ST-43), backend tests in both modes (ST-03), the
  mock service retired (ST-02), every command's permission checked by a
  test (ST-44), the experiment's leftovers gone (ST-33), one UiGraph mapper
  (ST-34).
- **Studio viewer parity**: ST-39. Done: a
  packet size the user sets (ST-22, seen on hardware), greying out what the
  camera refuses (ST-35), the stream description following a mid-stream
  format change (ST-17), "Generate report" (ST-41), dropped features named
  in Studio and Python (DX-05), dropped frames shown (ST-36), the camera's
  information, persistent IP and ForceIP (ST-37), user sets (ST-38), a Feature
  Browser that reads only what is on screen and works from the keyboard
  (ST-40), USB3 Vision cameras in embedded mode (ST-14).
- **Studio distribution**: ST-07. Done: unsigned bundles on every release
  tag, versioned with the library (ST-06).
- **Later**: ST-08, ST-09, ST-10, ST-11, ST-12, ST-13, ST-42.
