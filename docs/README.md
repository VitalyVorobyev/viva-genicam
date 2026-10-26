# Developer documentation

For people working on viva-genicam. Using it? The
[user guide](https://vitalyvorobyev.github.io/viva-genicam/) is the place to
start; [`CONTRIBUTING.md`](../CONTRIBUTING.md) covers building, testing and
pull requests.

| Document | What it holds |
|---|---|
| [`design.md`](design.md) | Crate map, key abstractions, data flows and the design tenets |
| [`standards.md`](standards.md) | GenICam and transport standards implemented here, their administrators, and the responsible crates |
| [`legal-review.md`](legal-review.md) | Licensing boundaries, source provenance, unresolved A3 questions, and distribution notices |
| [`adrs/`](adrs/README.md) | Architecture decision records: why the big choices were made |
| [`roadmap.md`](roadmap.md) | The order of the open work, by backlog ID |
| [`backlog.md`](backlog.md) | Actionable tasks, each with its evidence |
| [`legal-review.md`](legal-review.md) | Licensing boundaries, provenance audit, distribution notices and unresolved A3 questions |
| [`performance-2026-10-26.md`](performance-2026-10-26.md) | Acquisition and frame-delivery CPU measurements, synthetic stress limits and Manta ROI tests |
| [`hardware-test-plan.md`](hardware-test-plan.md) | Exercising every feature against a real GigE Vision camera, and which backlog rows each area settles |
| [`studio/zenoh-api.md`](studio/zenoh-api.md) | The Zenoh contract between the services and Viva Studio, and any other client |
| [`studio/testing-cookbook.md`](studio/testing-cookbook.md) | Running and testing Viva Studio without a camera |
| [`studio/manual-e2e-test.md`](studio/manual-e2e-test.md) | The manual end-to-end checklist for Studio, against a fake or real camera |

Viva Studio's own layout, data model and checks are in
[`../studio/README.md`](../studio/README.md). What shipped, and why, is in the
[changelog](../CHANGELOG.md).
