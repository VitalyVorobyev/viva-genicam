# GenICam & Vision Standards Primer

This chapter orients you in the standards behind GigE Vision and USB3 Vision cameras: skim the concepts, then jump to the tutorials.

## 1) Control vs. Data paths (big picture)
- **Control**: configure the device, read status, fetch the GenApi XML. In GigE Vision, control is **GVCP** (GigE Vision Control Protocol, UDP): register and memory reads and writes. GVCP is a protocol of its own that **GenCP** (Generic Control Protocol) generalised — the same ideas and a shared core of status codes, but its own opcodes, headers and big-endian fields. USB3 Vision carries GenCP itself.
- **Data**: receive image/metadata stream(s). In GigE Vision, data is **GVSP** (GigE Vision Streaming Protocol, UDP), typically one-way from camera → host.
- **Events & Actions**: GVCP supports device→host events and host→device action commands for sync/triggering.

```
   +------------------------+        +--------------------+
   |        Host            |        |     Camera         |
   |  (this repository)     |        |  (GigE Vision)     |
   +-----------+------------+        +----------+---------+
               | GVCP (UDP, control)           |
               |  registers, memory, XML       |
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
- **UDP** packets carry payloads (image data/metadata). The host reassembles frames, chooses a **packet size** that fits the path MTU, and may introduce **packet delay** to avoid NIC/driver overflow.
- **Chunks**: optional metadata blocks (e.g., `Timestamp`, `ExposureTime`) can be enabled and parsed alongside image data.
- **Time mapping**: devices often use **tick counters**; the host maintains a mapping between device ticks and host time for cross‑correlation.

## 4) How standards map to crates
Each layer of the standards is a crate of its own; [How the library fits together](architecture.md) shows which.

## 5) USB3 Vision
- Similar split between control and data paths, but with **USB3** transport (`viva-u3v`): GenCP over USB bulk endpoints for control, and a separate bulk endpoint for image data. Discovery is USB enumeration rather than a broadcast. The higher‑level GenApi and NodeMap concepts remain the same.

## 6) What to read next
- [How the library fits together](architecture.md) for the layers, which one to use, and what blocks.
- [Crates](crates/README.md) for the transport and GenApi deep dives.
- [Tutorials](tutorials/README.md) to configure features and receive frames end‑to‑end.
