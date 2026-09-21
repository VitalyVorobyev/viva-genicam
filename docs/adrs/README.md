# Architecture Decision Records

Decisions that shaped viva-genicam, in the classical ADR format
(Context / Decision / Consequences). Add a new ADR whenever an architectural
decision is made — retrospective ADRs for past decisions are welcome too.

Numbers **0001–0010** predate this repository and were imported with the studio,
so they describe a GenTL-based external service that no longer exists. Three of
them are marked below and carry a "What changed" note explaining what replaced
them; a superseded ADR is kept rather than deleted, because the reversal is part
of the record.

| ADR | Title | Status |
|-----|-------|--------|
| [0001](adr0001-desktop-primary.md) | Desktop-primary with WASM maintenance mode | Partly superseded (WASM runtime gone) |
| [0002](adr0002-camera-service-architecture.md) | Camera service as library + Zenoh process (external) | **Superseded** by 0011 / 0012 / 0017 |
| [0003](adr0003-gentl-transport.md) | GenTL as sole transport abstraction | **Superseded** by 0011 |
| [0004](adr0004-single-camera-scope.md) | Single-camera connection model | Accepted |
| [0005](adr0005-pixel-format-support.md) | Full SFNC pixel format coverage | Accepted |
| [0006](adr0006-progressive-disclosure-ui.md) | Progressive disclosure for Image Viewer controls | Accepted |
| [0007](adr0007-configurable-sfnc-groups.md) | Configurable SFNC feature groups in Image Viewer | Accepted |
| [0008](adr0008-zenoh-api-contract.md) | Zenoh key-expression API contract | Accepted |
| [0009](adr0009-uigraph-json-contract.md) | UiGraph as the single UI data contract | Accepted |
| [0010](adr0010-feature-state-contract.md) | FeatureState as the authoritative live-state contract | Accepted |
| [0011](adr0011-pure-rust-genicam-stack.md) | Pure-Rust GenICam Stack | Accepted |
| [0012](adr0012-layered-crate-architecture.md) | Layered Crate Architecture with a Single Workspace Version | Accepted |
| [0013](adr0013-fake-camera-first-testing.md) | Fake-Camera-First Testing and the Realism Policy | Accepted |
| [0014](adr0014-sync-registerio-async-adapters.md) | Synchronous RegisterIo with Async Adapters | Accepted |
| [0015](adr0015-vendored-libusb-lgpl-notices.md) | Vendored libusb in PyPI Wheels with LGPL Notices | Accepted |
| [0016](adr0016-cargo-deny-single-gate.md) | cargo-deny as the Single Supply-Chain Gate | Accepted |
| [0017](adr0017-studio-monorepo-two-workspaces.md) | Studio Monorepo with Two Cargo Workspaces | Accepted |
| [0018](adr0018-genapi-conformance-over-convenience.md) | GenApi Conformance over Convenient Approximations | Accepted |
| [0019](adr0019-transport-conformance-and-spec-derived-fakes.md) | Transport Conformance and Spec-Derived Fakes | Accepted |
| [0020](adr0020-per-transport-status-codes.md) | Per-Transport Status Codes over a Shared Table | Proposed |
| [0021](adr0021-gvsp-packet-size-policy.md) | GVSP Packet-Size Policy (Preserve, `--auto`, Explicit) | Accepted |
| [0022](adr0022-integer-register-decoding.md) | Reinterpret Full-Width Unsigned Registers Rather Than Refuse Them | Accepted |

**Template:** `# ADR-NNNN: Title`, `**Status:**`, `**Date:**`, `## Context`,
`## Decision`, `## Consequences` (Positive/Negative). File name:
`adrNNNN-kebab-title.md`.
