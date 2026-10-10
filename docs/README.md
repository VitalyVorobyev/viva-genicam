# Developer documentation

For people working on viva-genicam. Using it? The
[user guide](https://vitalyvorobyev.github.io/viva-genicam/) is the place to
start; [`CONTRIBUTING.md`](../CONTRIBUTING.md) covers building, testing and
pull requests.

| Document | What it holds |
|---|---|
| [`design.md`](design.md) | Crate map, key abstractions, data flows and the design tenets |
| [`standards.md`](standards.md) | The EMVA standards implemented here — GenApi, GenCP, GVCP/GVSP, USB3 Vision, SFNC, PFNC — and which crate implements which |
| [`adrs/`](adrs/README.md) | Architecture decision records: why the big choices were made |
| [`roadmap.md`](roadmap.md) | Mid-term direction, by phase |
| [`backlog.md`](backlog.md) | Actionable tasks, each with its evidence |
| [`studio/zenoh-api.md`](studio/zenoh-api.md) | The Zenoh contract between the services and Viva Studio, and any other client |
| [`studio/testing-cookbook.md`](studio/testing-cookbook.md) | Running and testing Viva Studio without a camera |
| [`studio/manual-e2e-test.md`](studio/manual-e2e-test.md) | The manual end-to-end checklist for Studio, against a fake or real camera |

Viva Studio's own layout, data model and checks are in
[`../studio/README.md`](../studio/README.md). What shipped, and why, is in
[`../CHANGELOG.md`](../CHANGELOG.md).
