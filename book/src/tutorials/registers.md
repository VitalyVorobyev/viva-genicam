# Registers & features

Goal of this tutorial:

- Read and write **GenApi features** such as `ExposureTime` or `Gain`.
- Understand how features map to the underlying **registers**.
- Use **selectors** (e.g. `GainSelector`) and understand what they change.
- Do all of it from both the `viva-camctl` CLI and Rust.

Work through [Discovery](./discovery.md) first, so you know your camera's IP and
which host interface you are using.

---

## Concepts: features vs registers

GenICam describes camera configuration as **features** in the GenApi XML:

- A feature has a name (`ExposureTime`, `Gain`, `PixelFormat`, …) and a type
  (Integer, Float, Boolean, Enumeration, Command, String, …).
- Under the hood, a feature usually corresponds to one or more **registers**. A
  simple one reads a single 32-bit register; others are derived through
  **SwissKnife** expressions, or depend on **selectors**.

The layering:

- `viva-genapi-xml` parses the XML into an `XmlModel`.
- `viva-genapi` builds a `NodeMap` from it and evaluates nodes on demand.
- `viva-genicam` and `viva-camctl` sit on top and hide the addressing.

---

## Step 1 – Inspect features with `viva-camctl`

You need the camera IP from the discovery tutorial, and the host interface IP if
you have several NICs.

### 1.1. Read a feature by name

```bash
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name ExposureTime
```

For machine-readable output, note that `--json` is a top-level flag and goes
**before** the subcommand:

```bash
cargo run -p viva-camctl -- --json get --ip 192.168.0.10 --name ExposureTime
```

### 1.2. Write a feature by name

```bash
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name ExposureTime
```

If the value does not change, the usual causes are:

- The feature is **locked right now**. GenApi expresses this with `pIsLocked`,
  which the library evaluates before every write — so a locked feature is
  refused locally with a clear error, rather than being sent to the camera and
  failing there.
- The value violates a constraint (range, increment, alignment).
- Another feature is overriding manual control — `ExposureAuto` is the usual
  culprit for `ExposureTime`, `GainAuto` for `Gain`.

### 1.3. Which features does this camera have?

The report bundle lists node and feature counts along with the XML itself:

```bash
cargo run -p viva-camctl -- report --ip 192.168.0.10 --out viva-report.txt
```

---

## Step 2 – Work with selectors

Many cameras multiplex several logical settings onto the same registers:

- `GainSelector` = `All`, `Red`, `Green`, `Blue`, …
- `Gain` = the value for the currently selected channel.

Changing the selector changes which "row" you are editing. The `NodeMap`
re-resolves the addressing and invalidates the cached values that depended on
it, so a read after a selector write returns the new channel's value rather than
a stale one.

`viva-camctl get` reads the selector's current value only. To see which
entries it accepts, call `Camera::enum_entries("GainSelector")` in Rust or
`cam.enum_entries("GainSelector")` in [Python](../python/control.md#enum-entries),
or look the node up in the XML that `viva-camctl xml` dumps.

```bash
# Which channel is selected now?
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name GainSelector

# Different gain per channel
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name GainSelector --value Red
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name Gain --value 5.0

cargo run -p viva-camctl -- set --ip 192.168.0.10 --name GainSelector --value Blue
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name Gain --value 3.0
```

The `selectors_demo` example shows the same pattern in Rust:

```bash
cargo run -p viva-genicam --example selectors_demo
```

---

## Step 3 – Do the same from Rust

```bash
cargo run -p viva-genicam --example get_set_feature
cargo run -p viva-genicam --example get_set_feature -- --name Gain --value 3.0
```

The whole of it:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/get_set_feature.rs:get_set}}
```

Two things are worth pointing out.

**Values are strings at this boundary.** `Camera::get` returns `String` and
`Camera::set` takes `&str`; the node's own type decides how that text is parsed
and encoded. `set_exposure_time_us` and `set_gain_db` are typed conveniences
over the two most common cases.

**Feature access is synchronous, including inside `#[tokio::main]`.** The
register I/O behind it blocks, and `GigeRegisterIo` steps off the async worker
by itself — you do not need to wrap calls in `spawn_blocking`.

If you have no camera to hand, the same calls work against the
[fake camera](./fake-camera.md):

```bash
cargo run -p viva-genicam --example demo_fake_camera
```

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:read_features}}
```

---

## Step 4 – When you might need raw register access

Prefer features by name. You get the node's type, you respect the vendor's
declared constraints, and your code stays portable across cameras.

Raw registers are still occasionally the right tool:

- Debugging unusual vendor behaviour or firmware bugs.
- Reaching something genuinely absent from the XML.
- Bringing up a device whose GenApi description is incomplete.

`viva-gige` and `viva-gencp` expose the primitives — see the
[viva-gige](../crates/viva-gige.md) and [viva-gencp](../crates/viva-gencp.md)
chapters. Be careful: writing arbitrary registers can leave a device unusable
until it is power-cycled.

---

## Recap

You should now be able to:

- Read and write features by name, from the CLI and from Rust.
- Recognise why a write was refused — a lock, a constraint, or an auto feature.
- Use selectors to address per-channel settings.
- Know that raw register access exists and is a last resort.

Next: [GenApi XML](./genapi-xml.md) — where the feature list comes from in the
first place.
