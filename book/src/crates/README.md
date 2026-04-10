# Crates overview

The `genicam-rs` workspace is split into small crates that mirror the structure of
the GenICam ecosystem:

- **Protocols & transport** (GenCP, GVCP/GVSP)
- **GenApi XML loading & evaluation**
- **Public “facade” API** for applications
- **Command-line tooling** for everyday camera work

This chapter is the “map of the territory”. It tells you *which* crate to use
for a given task, and where to look if you want to hack on internals.

---

## Quick map

| Crate       | Path                   | Role / responsibility                                             | Primary audience                  |
|-------------|------------------------|--------------------------------------------------------------------|-----------------------------------|
| `viva-gencp`    | `crates/viva-gencp`        | GenCP encode/decode + helpers for control path over GVCP         | Contributors, protocol nerds      |
| `viva-gige`   | `crates/viva-gige`       | GigE Vision transport: GVCP (control) + GVSP (streaming)          | End-users & contributors          |
| `viva-genapi-xml`| `crates/genapi-xml`    | Load GenICam XML from device / disk, parse into IR                | Contributors (XML / SFNC work)    |
| `viva-genapi` | `crates/viva-genapi` | NodeMap implementation, feature access, SwissKnife, selectors     | End-users & contributors          |
| `viva-genicam`   | `crates/genicam`       | High-level “one crate” façade combining transport + GenApi        | End-users                         |
| `viva-camctl` | `crates/viva-camctl`     | CLI tool for discovery, configuration, streaming, benchmarks      | End-users, ops, CI scripts        |

If you just want to **use a camera** from Rust, you’ll usually start with
`viva-genicam` (or `viva-camctl` from the command line) and ignore the lower layers.

---

## How the crates fit together

At a high level, the crates compose like this:

```text
           ┌───────────────┐      ┌────────────────┐
           │   viva-gencp      │      │   viva-genapi  │
           │ GenCP encode  │      │ NodeMap,       │
           │ / decode      │      │ SwissKnife,    │
           └─────┬─────────┘      │ selectors      │
                 │                └──────┬─────────┘
                 │                       │
           ┌─────▼─────────┐      ┌──────▼─────────┐
           │   viva-gige     │      │  viva-genapi-xml    │
           │ GVCP / GVSP   │      │ XML loading &  │
           │ packet I/O    │      │ schema-lite IR │
           └─────┬─────────┘      └──────┬─────────┘
                 │                       │
                 └──────────┬────────────┘
                            │
                      ┌─────▼─────┐
                      │  genicam  │  ← public Rust API
                      └─────┬─────┘
                            │
                      ┌─────▼───────┐
                      │ viva-camctl   │  ← CLI on top of `viva-genicam`
                      └─────────────┘
```

Roughly:
* `viva-gige` knows how to talk UDP to a GigE Vision device (discovery, register
access, image packets, resends, stats, …).
* `viva-gencp` provides the GenCP building blocks used on the control path.
* `viva-genapi-xml` fetches and parses the GenApi XML that describes the device’s
features.
* `viva-genapi` turns that XML into a NodeMap you can read/write, including
SwissKnife expressions and selector-dependent features.
* `viva-genicam` stitches all of the above into a reasonably ergonomic API.
* `viva-camctl` exposes common workflows from genicam as `cargo run -p viva-camctl -- …`.

⸻

## When to use which crate

### I just want to use my camera from Rust

Use `viva-genicam`.

Typical tasks:
* Enumerate cameras on a NIC
* Open a device, read/write features by name
* Start a GVSP stream, iterate over frames, look at stats
* Subscribe to events or send action commands

Start with the examples under `crates/viva-genicam/examples/` and the [Tutorials](../tutorials/README.md).

⸻

### I want a command-line tool for daily work

Use `viva-camctl`.

Typical tasks:
* Discovery: list all cameras on a given interface
* Register/feature inspection and configuration
* Quick streaming tests and stress benchmarks
* Enabling/disabling chunk data, configuring events

This is also a good reference for how to structure a “real” application on top
of genicam.

⸻

### I need to touch GigE Vision packets / low-level transport

Use `viva-gige` (and `viva-gencp` as needed).

Example reasons:
* You want to experiment with MTU, packet delay, resend logic, or custom stats
* You’re debugging interoperability with a weird device and need raw GVCP/GVSP
* You want to build a non-GenApi tool that only tweaks vendor-specific registers

The [`viva-gige` chapter](./viva-gige.md) goes into more detail on discovery,
streaming, events, actions, and tuning.

⸻

### I want to work on GenApi / XML internals

Use `viva-genapi-xml` and `viva-genapi`.

Typical contributor activities:
* Supporting new SFNC features or vendor extensions
* Improving SwissKnife coverage or selector handling
* Adding tests for tricky XML from specific camera families

The following chapters are relevant:
* [GenApi XML loader: genapi-xml](./genapi-xml.md)
* [GenApi core & NodeMap: viva-genapi](./viva-genapi.md)

If you’re not sure where a GenApi bug lives, the rule of thumb is:
* “XML can’t be parsed” → genapi-xml
* “Feature exists but behaves wrong” → viva-genapi
* “Device returns odd data / status codes” → viva-gige or viva-gencp

⸻

### I need a single high-level entry point

Use `viva-genicam`.

This crate aims to expose just enough control/streaming surface for most applications without making you think about transports, XML, or NodeMap internals.

The [genicam crate chapter](./genicam.md)￼ shows:
* How to go from “no camera” to “frames in memory” in ~20 lines
* How to query and set features safely (with proper types)
* How to plug in your own logging, error handling, and runtime

⸻

## Crate deep dives

The rest of this section of the book contains crate-specific chapters:
* [GenCP: viva-gencp](./viva-gencp.md)￼– control protocol building blocks.
* [GigE Vision transport: `viva-gige`](./viva-gige.md)￼– discovery, streaming, events, actions.
* [GenApi XML loader: `viva-genapi-xml`](./genapi-xml.md)￼– getting from device to IR.
* [GenApi core & NodeMap: `viva-genapi`](./viva-genapi.md) – evaluating features, including SwissKnife.
* [Facade API: `viva-genicam`](./genicam.md)￼– the crate most end-users start with.
* [Future / helper crates](./placeholders.md) – notes on planned additions.

If you’re reading this for the first time, a good path is:
1. Skim this page.
2. Read the [genicam](./genicam.md) chapter.
3. Jump to viva-gige or viva-genapi when you hit something you want to tweak.
