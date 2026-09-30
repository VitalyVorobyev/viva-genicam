# Architecture Overview

This section maps the runtime flow, crate boundaries, and key traits so both app developers and contributors can reason about the system.

## Layered view

```
+---------------------------+   End‑user API & examples
|   viva-genicam (façade)   |   - device discovery, feature get/set
|   (+ examples/)           |   - StreamBuilder / FrameStream, events
+-------------+-------------+
|
v
+---------------------------+   GenApi core
|   viva-genapi             |   - Node types (Integer/Float/Enum/Bool/Command,
|                           |     Register, String, **SwissKnife**)
|                           |   - NodeMap build & evaluation
|                           |   - Selector routing & dependency graph
+-------------+-------------+
|
v
+---------------------------+   GenApi XML
|   viva-genapi-xml         |   - Fetch XML via control path
|                           |   - Parse schema‑lite → IR used by viva-genapi
+-------------+-------------+
|
v
+---------------------------+   Transports
|   viva-gige               |   - GVCP (control): discovery, read/write, events,
|                           |     action commands
|                           |   - GVSP (data): packet parsing, reassembly,
|                           |     MTU/packet size, delay, stats
+-------------+-------------+
|
v
+---------------------------+   Protocol helpers
|   viva-gencp              |   - GenCP encode/decode, status codes, helpers
+---------------------------+
```

## Data flow
1. **Discovery** (`viva-gige`): bind to NIC → broadcast GVCP discovery → parse replies.
2. **Connect**: establish control channel (UDP) and prepare stream endpoints if needed.
3. **GenApi XML** (`viva-genapi-xml`): read address from device registers → fetch XML → parse to IR.
4. **NodeMap** (`viva-genapi`): build nodes, resolve links (Includes, Pointers, Selectors), set defaults.
5. **Evaluation** (`viva-genapi`):
   - **Direct** nodes read/write underlying registers via `viva-gige` + `viva-gencp`.
   - **Computed** nodes (e.g., **SwissKnife**) evaluate expressions that reference other nodes.
6. **Streaming** (`viva-gige`): configure packet size/delay → receive GVSP → reassemble → expose frames + **chunks** and **timestamps**.

## Async, threading, and I/O
- Discovery, stream setup and GVSP reception use async UDP sockets (Tokio). `FrameStream::next_frame` is an `async fn`; statistics accumulate as frames complete.
- Node evaluation is **synchronous**: the `RegisterIo` trait `NodeMap` reads and writes through is a plain blocking interface, and `Camera::get`/`set` block until the register transaction completes. `GigeRegisterIo` bridges that onto the async GVCP client itself, so calling them inside `#[tokio::main]` is fine. The reasoning is in [ADR-0014](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0014-sync-registerio-async-adapters.md).

## Error handling & tracing
- Each layer has its own error type (`GigeError`, `GenCpError`, `XmlError`, `GenApiError`, and `GenicamError` at the façade); see [Error Handling & Logging](errors-logging.md).
- Enable logs with `RUST_LOG=info` (or `debug`, `trace`).

## Platform considerations
- **Windows/Linux/macOS** supported. Windows firewall and NIC settings are covered in [Networking → Windows](networking.md#22-windows).
- Multi‑NIC hosts should explicitly select the interface for discovery/streaming.

## Extending the system
- Add node kinds in `viva-genapi` as a new `Node` enum variant with its evaluation and dependency wiring, and make anything unsupported land in `NodeMap::skipped()` rather than failing the document.
- Add transports as new `viva-*` crates behind a trait the facade can select at runtime.
- Keep `viva-genicam` thin: compose transport + NodeMap + utilities; keep heavy logic in lower crates.
