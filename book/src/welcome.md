# Welcome & Goals

**genicam-rs** provides *pure Rust* building blocks for the GenICam ecosystem with an Ethernet-first focus (GigE Vision) and **first-class support for Windows, Linux, and macOS**.

## Who is this book for?
- **End‑users** building camera applications who want a practical high‑level API and copy‑pasteable examples.
- **Contributors** extending transports, GenApi features (incl. expression nodes), and streaming—who need a clear mental model of crates and internal boundaries.

## What works today
- Device **discovery** over GigE Vision (GVCP) on a selected network interface.
- **Control path**: reading/writing device memory via GenCP over GVCP; fetching the device’s GenApi XML.
- **GenApi**: NodeMap with common node kinds (Integer/Float/Enum/Bool/Command/**SwissKnife**), ranges, access modes, and selector-based addressing.
- **CLI** (`viva-camctl`) for common operations: discovery, feature get/set, streaming, events, chunks, and benchmarks.
- **Streaming (GVSP)**: packet reassembly, resend handling, MTU/packet sizing & delay, and stats (evolving).

> Details evolve fast—check examples and release notes for the latest capabilities.

## What’s coming next
- Additional GenApi nodes (e.g., Converter, complex formulas), dependency evaluation/caching improvements.
- USB3 Vision transport.
- GenTL producer (.cti) and PFNC/SFNC coverage.

## How this book is organized
- Start with **Quick Start** to build, test, and run the first discovery.
- Read the **Primer** and **Architecture** to get the big picture.
- Use **Crate Guides** and **Tutorials** for hands‑on tasks.
- See **Networking** and **Troubleshooting** when packets don’t behave.
