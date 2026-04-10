# GenICam & Vision Standards Primer

This chapter orients you in the standards and shows how they map to the crates in this repo. If you’re an end‑user, skim the concepts and jump to tutorials. If you’re a contributor, the mappings help you navigate the code.

## 1) Control vs. Data paths (big picture)
- **Control**: configure the device, read status, fetch the GenApi XML. In GigE Vision, control is **GVCP** (GigE Vision Control Protocol, UDP) carrying **GenCP** (Generic Control Protocol) semantics for register reads/writes and feature access.
- **Data**: receive image/metadata stream(s). In GigE Vision, data is **GVSP** (GigE Vision Streaming Protocol, UDP), typically one-way from camera → host.
- **Events & Actions**: GVCP supports device→host events and host→device action commands for sync/triggering.

```
   +------------------------+        +--------------------+
   |        Host            |        |     Camera         |
   |  (this repository)     |        |  (GigE Vision)     |
   +-----------+------------+        +----------+---------+
               | GVCP (UDP, control)           |
               |  GenCP (registers/features)   |
               v                                ^
       Configure, query, XML                    |
                                                |
               ^                                v
               | GVSP (UDP, data)         Image/Chunks
               | (streaming)                    |
```

## 2) GenApi XML & NodeMap
- The device exposes an **XML description** of its features (nodes). Nodes form a graph with types like **Integer**, **Float**, **Boolean**, **Enumeration**, **Command**, **String**, **Register**, and expression nodes like **SwissKnife**.
- Nodes have **AccessMode** (RO/RW), **Visibility** (Beginner/Expert/Guru), **Units**, **Min/Max/Inc**, **Selector** links, and **Dependencies** (i.e., a node’s value depends on other nodes).
- The host builds a **NodeMap** from the XML and **evaluates** nodes on demand: some read/write device registers; others compute values from expressions.

### SwissKnife (implemented)
- A **SwissKnife** node computes its value from an expression referencing other nodes (e.g., arithmetic, logic, conditionals). Typical uses:
  - Derive human‑readable features from raw register fields.
  - Apply scale/offset and conditionals depending on selectors.
- In this project, SwissKnife is **evaluated in the NodeMap**, so reads of dependent nodes trigger the calculation transparently.

### Selectors
- Selectors (e.g., `GainSelector`) change the **addressing** or **active branch** so the same feature name maps to different underlying registers or computed paths.

## 3) Streaming: GVSP
- **UDP** packets carry payloads (image data/metadata). The host reassembles frames, handles **resend** requests, negotiates **packet size/MTU**, and may introduce **packet delay** to avoid NIC/driver overflow.
- **Chunks**: optional metadata blocks (e.g., `Timestamp`, `ExposureTime`) can be enabled and parsed alongside image data.
- **Time mapping**: devices often use **tick counters**; the host maintains a mapping between device ticks and host time for cross‑correlation.

## 4) How standards map to crates
| Concept | Crate | Responsibility |
|---|---|---|
| GenCP (encode/decode, status) | `viva-gencp` | Message formats, errors, helpers for control-path operations |
| GVCP/GVSP (GigE Vision) | `viva-gige` | Discovery, control channel, streaming engine, resend/MTU/delay, events/actions |
| GenApi XML loader | `viva-genapi-xml` | Fetch XML via control path and parse schema‑lite into an internal representation |
| NodeMap & evaluation | `viva-genapi` | Node types (incl. **SwissKnife**), dependency resolution, selector routing, value get/set |
| Public façade | `viva-genicam` | End‑user API combining transport + NodeMap + utilities (examples live here) |

## 5) USB3 Vision (preview)
- Similar split between control and data paths, but with **USB3** transport and different discovery/endpoint mechanics. The higher‑level GenApi and NodeMap concepts remain the same.

## 6) What to read next
- **Architecture Overview** for a code‑level view of modules, traits, and async/concurrency.
- **Crate Guides** for deep dives (APIs, examples, edge cases).
- **Tutorials** to configure features and receive frames end‑to‑end.
