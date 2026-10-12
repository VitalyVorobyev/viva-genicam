# Crates overview

Every crate, and the layer it implements, is on
[How the library fits together](../architecture.md). This part of the book says
which one to reach for, and goes deeper into three of them.

---

## When to use which crate

### I just want to use my camera from Rust

Use `viva-genicam`.

Typical tasks:
* Enumerate cameras on a NIC
* Open a device, read/write features by name
* Start a GVSP stream, iterate over frames, look at stats
* Subscribe to events or send action commands

Start with the examples under `crates/viva-genicam/examples/` and the
[Tutorials](../tutorials/README.md): they go from discovery to frames in
memory, and show how to query and set features by name, and why a write can be
refused.

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

### I want to use my camera from software that loads GenTL producers

Use `viva_gentl.cti`, built from the `viva-gentl` crate and attached to each
release: Harvesters and other GenTL consumers load it as they load a vendor's
`.cti` ([Use viva from other tools](../gentl.md)).

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

### I want to understand how a feature becomes register access

Read the [`viva-genapi` chapter](./viva-genapi.md): addressing, selectors,
SwissKnife formulas, access modes and caching. If a feature misbehaves, the
rule of thumb is:
* “XML can’t be parsed” → `viva-genapi-xml`
* “Feature exists but behaves wrong” → `viva-genapi`
* “Device returns odd data / status codes” → `viva-gige` or `viva-gencp`

---

## Crate deep dives

* [GenCP: `viva-gencp`](./viva-gencp.md) – control protocol building blocks.
* [GigE Vision transport: `viva-gige`](./viva-gige.md) – discovery, streaming, events, actions.
* [GenApi core & NodeMap: `viva-genapi`](./viva-genapi.md) – evaluating features, including SwissKnife.
