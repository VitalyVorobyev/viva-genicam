# Architecture Overview

This section maps the runtime flow, crate boundaries, and key traits so both app developers and contributors can reason about the system.

## Layered view

```
+---------------------------+   End‑user API & examples
|   genicam (façade)        |   - device discovery, feature get/set
|   crates/viva-genicam/examples |   - streaming helpers, CLI wiring
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
|                           |   - GVSP (data): receive, reassembly, resend,
|                           |     MTU/packet size negotiation, delay, stats
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
- Transport uses async UDP sockets (Tokio) and bounded channels for back‑pressure.
- Frame reassembly runs on dedicated tasks; statistics are aggregated periodically.
- Node evaluation is sync from the caller’s perspective; I/O hops are awaited within accessors.

## Error handling & tracing
- Errors are categorized by layer (transport/protocol/genapi/eval). Use `anyhow`/custom error types at boundaries.
- Enable logs with `RUST_LOG=info` (or `debug`,`trace`) and consider JSON output for tooling.

## Platform considerations
- **Windows/Linux/macOS** supported. On Windows, run discovery once as admin to authorize firewall; consider jumbo frames per NIC for high FPS.
- Multi‑NIC hosts should explicitly select the interface for discovery/streaming.

## Extending the system
- Add nodes in `viva-genapi` by implementing the evaluation trait and wiring dependencies.
- Add transports as new `viva-*` crates behind a trait the facade can select at runtime.
- Keep `viva-genicam` thin: compose transport + NodeMap + utilities; keep heavy logic in lower crates.
