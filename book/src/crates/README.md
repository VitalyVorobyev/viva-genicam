# Crates overview

The `viva-genicam` workspace is split into small crates that mirror the structure of
the GenICam ecosystem:

- **Protocols & transport** (GenCP, GVCP/GVSP)
- **GenApi XML loading & evaluation**
- **Public “facade” API** for applications
- **Command-line tooling** for everyday camera work

This chapter is the “map of the territory”. It tells you *which* crate to use
for a given task, and where to look if you want to hack on internals.

---

## Quick map

Every crate lives under `crates/` and is named for its directory.

| Crate | Role / responsibility | Primary audience |
|---|---|---|
| `viva-genicam` | High-level facade combining transport + GenApi | **End-users — start here** |
| `viva-camctl` | CLI: discovery, features, streaming, diagnostics, IP config | End-users, ops, CI scripts |
| `viva-pygenicam` | Python bindings (PyO3). Own workspace, own `Cargo.lock` | Python users |
| `viva-genapi` | NodeMap, feature access, SwissKnife, selectors, access predicates | End-users & contributors |
| `viva-genapi-xml` | Load GenICam XML from device or disk, parse into `XmlModel` | Contributors (XML / SFNC work) |
| `viva-gige` | GigE Vision transport: GVCP control + GVSP streaming | End-users & contributors |
| `viva-u3v` | USB3 Vision transport over libusb | End-users & contributors |
| `viva-gencp` | GenCP message encode/decode, shared by both transports | Contributors, protocol nerds |
| `viva-pfnc` | Pixel Format Naming Convention tables | Contributors |
| `viva-sfnc` | Standard Feature Naming Convention constants | Contributors |
| `viva-zenoh-api` | Message payloads and topic names shared with Viva Studio; plain serde types that do not link Zenoh | Contributors |
| `viva-service` | Zenoh bridge: GigE cameras → Viva Studio | Integrators |
| `viva-service-u3v` | Zenoh bridge: U3V cameras → Viva Studio | Integrators |
| `viva-fake-gige` | In-process fake GigE camera for tests and demos | Everyone |
| `viva-fake-u3v` | In-process fake U3V camera for tests | Contributors |

If you just want to **use a camera** from Rust, start with `viva-genicam` (or
`viva-camctl` from the command line) and ignore the lower layers. Three of these
have chapters of their own: [`viva-gige`](viva-gige.md),
[`viva-genapi`](viva-genapi.md) and [`viva-gencp`](viva-gencp.md).

---

## How the crates fit together

At a high level, the crates compose like this:

```text
   ┌───────────────┐        ┌────────────────┐
   │  viva-gencp   │        │  viva-genapi   │
   │ GenCP encode  │        │ NodeMap,       │
   │ / decode      │        │ SwissKnife,    │
   └───────┬───────┘        │ selectors      │
           │                └───────┬────────┘
           │                        │
   ┌───────▼───────┐        ┌───────▼────────┐
   │  viva-gige /  │        │viva-genapi-xml │
   │  viva-u3v     │        │ XML loading &  │
   │ transports    │        │ parsing        │
   └───────┬───────┘        └───────┬────────┘
           │                        │
           └───────────┬────────────┘
                       │
               ┌───────▼───────┐
               │ viva-genicam  │  ← public Rust API
               └───────┬───────┘
                       │
               ┌───────▼───────┐
               │ viva-camctl   │  ← CLI on top of viva-genicam
               └───────────────┘
```

Roughly:
* `viva-gige` knows how to talk UDP to a GigE Vision device (discovery, register
access, image packets, stats, …).
* `viva-gencp` provides the GenCP building blocks used on the control path.
* `viva-genapi-xml` fetches and parses the GenApi XML that describes the device’s
features.
* `viva-genapi` turns that XML into a NodeMap you can read/write, including
SwissKnife expressions and selector-dependent features.
* `viva-genicam` stitches all of the above into a reasonably ergonomic API.
* `viva-camctl` exposes common workflows from `viva-genicam` on the command line.

---

## When to use which crate

### I just want to use my camera from Rust

Use `viva-genicam`.

Typical tasks:
* Enumerate cameras on a NIC
* Open a device, read/write features by name
* Start a GVSP stream, iterate over frames, look at stats
* Subscribe to events or send action commands

Start with the examples under `crates/viva-genicam/examples/` and the [Tutorials](../tutorials/README.md).

---

### I want a command-line tool for daily work

Use `viva-camctl`.

Typical tasks:
* Discovery: list all cameras on a given interface
* Register/feature inspection and configuration
* Quick streaming tests and stress benchmarks
* Enabling/disabling chunk data, configuring events

This is also a good reference for how to structure a “real” application on top
of `viva-genicam`.

---

### I need to touch GigE Vision packets / low-level transport

Use `viva-gige` (and `viva-gencp` as needed).

Example reasons:
* You want to experiment with MTU, packet delay, or custom stats
* You’re debugging interoperability with a weird device and need raw GVCP/GVSP
* You want to build a non-GenApi tool that only tweaks vendor-specific registers

The [`viva-gige` chapter](./viva-gige.md) goes into more detail on discovery,
streaming, events, actions, and tuning.

---

### I want to work on GenApi / XML internals

Use `viva-genapi-xml` and `viva-genapi`.

Typical contributor activities:
* Supporting new SFNC features or vendor extensions
* Improving SwissKnife coverage or selector handling
* Adding tests for tricky XML from specific camera families

The following chapter is relevant:
* [GenApi core & NodeMap: viva-genapi](./viva-genapi.md)

If you’re not sure where a GenApi bug lives, the rule of thumb is:
* “XML can’t be parsed” → viva-genapi-xml
* “Feature exists but behaves wrong” → viva-genapi
* “Device returns odd data / status codes” → viva-gige or viva-gencp

---

### I need a single high-level entry point

Use `viva-genicam`.

This crate aims to expose just enough control/streaming surface for most applications without making you think about transports, XML, or NodeMap internals.

The [tutorials](../tutorials/README.md) show:
* How to go from discovery to frames in memory
* How to query and set features by name, and why a write can be refused

---

## Crate deep dives

The rest of this section of the book contains crate-specific chapters:
* [GenCP: `viva-gencp`](./viva-gencp.md) – control protocol building blocks.
* [GigE Vision transport: `viva-gige`](./viva-gige.md) – discovery, streaming, events, actions.
* [GenApi core & NodeMap: `viva-genapi`](./viva-genapi.md) – evaluating features, including SwissKnife.

If you’re reading this for the first time, a good path is:
1. Skim this page.
2. Work through the [tutorials](../tutorials/README.md).
3. Jump to viva-gige or viva-genapi when you hit something you want to tweak.
