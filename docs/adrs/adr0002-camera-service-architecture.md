# ADR-0002: Camera service as library + Zenoh process (external)

**Status:** Superseded by [ADR-0011](adr0011-pure-rust-genicam-stack.md),
[ADR-0012](adr0012-layered-crate-architecture.md) and
[ADR-0017](adr0017-studio-monorepo-two-workspaces.md)
**Date:** 2026-03-06

> **What changed.** Both premises are gone. The camera service is not external
> and not in a separate repository: it is `crates/viva-service` and
> `crates/viva-service-u3v` in this workspace, layered per ADR-0012, with Studio
> as the second workspace alongside it per ADR-0017. And it does not talk to
> cameras through GenTL — ADR-0011 replaced that with a pure-Rust GVCP/GVSP and
> USB3 Vision stack, so the "never depends on camera SDKs" boundary held in
> spirit and dissolved in form.
>
> What survives is the part that mattered: the **Zenoh API contract** is still
> the single integration point, still owned by `viva-zenoh-api`, and still
> specified in [`docs/studio/zenoh-api.md`](../studio/zenoh-api.md) — see
> [ADR-0008](adr0008-zenoh-api-contract.md). The separate
> `docs/camera-service-api.md` contract document this ADR names has been deleted;
> it described an API for an implementor who does not exist.

## Context

GenICam Studio needs to communicate with physical cameras. The architecture assumes an external camera service process that speaks to cameras and exposes data over Zenoh.

## Decision

The camera service is an **external component** that lives in a **separate repository**. It is implemented as a Rust library crate with a thin binary wrapper that runs it as a standalone Zenoh process.

GenICam Studio (this repo) owns:
- The **Zenoh API contract** (`viva-zenoh-api` crate, `docs/studio/zenoh-api.md`) that the service must implement
- The **Tauri desktop app** that consumes the service over Zenoh
- A **mock camera service** (`apps/genicam-mock-service`) for development and testing

The external camera service owns:
- GenTL interaction, camera SDK calls
- XML retrieval from physical devices
- Node read/write via register access
- Image acquisition and frame streaming
- The Zenoh bridge that maps its internal API to the Zenoh key-expression contract

## Consequences

- Clear repo boundary: GenICam Studio never depends on camera SDKs or GenTL.
- The Zenoh API contract is the single integration point between the two repos.
- The mock service enables full end-to-end development and CI without hardware.
