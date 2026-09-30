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

**Open**: TC-05, TC-06, TC-10, TC-11, TC-12, TC-16, TC-18, TC-21.

## Phase 2 — Diagnostics loop

**Goal**: every user can produce the artifact that diagnoses their camera, on a
camera we cannot open. Almost every fix so far came from a user-supplied
artifact, so producing one must be easy even when nothing is known to be wrong.

**Open**: DX-05, DX-06.

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

**Goal**: the services and Studio behave like a product — correct discovery
cadence, U3V parity, and a releasable desktop app.

- **Services**: SVC-01, SVC-03, SVC-04, SVC-05.
- **Real-service integration** (Studio against `viva-service`, not mocks): ST-02,
  ST-03, ST-14, ST-17, ST-18, ST-22.
- **Release prep** (packaging, service sidecar, annotations): ST-06, ST-07,
  ST-08, ST-09.
- **Recording & polish**: ST-10, ST-11, ST-12, ST-13.
- **Housekeeping**: ST-01.
