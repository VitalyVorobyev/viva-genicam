# Registers & features

Goal of this tutorial:

- Read and write **GenApi features** such as `ExposureTime` or `Gain`.
- Understand how features map to the underlying **registers**.
- Learn the basics of **selectors** (e.g. `GainSelector`) and how they affect values.
- See how to do the same thing from:
  - The `viva-camctl` CLI.
  - The `viva-genicam` Rust examples.

If you haven’t done so yet, first go through:

- [Discovery](./discovery.md)

so you know the IP of your camera and which host interface you’re using.

---

## Concepts: features vs registers

GenICam exposes camera configuration through **features** described in the
GenApi XML:

- A **feature** has a name (`ExposureTime`, `Gain`, `PixelFormat`, …).
- Each feature has a **type**:
  - Integer / Float / Boolean / Enumeration / Command, …
- Under the hood, a feature usually corresponds to one or more **registers**:
  - A simple feature may read/write a single 32-bit register.
  - More complex ones may be derived via **SwissKnife** expressions or depend
    on **selectors**.

The `viva-genapi` crate:

- Loads the XML (via `viva-genapi-xml`).
- Builds a **NodeMap**.
- Lets you read and write features by name using typed accessors.

The `viva-genicam` crate and `viva-camctl` CLI sit on top of this NodeMap and try to
hide most of the low-level details.

---

## Step 1 – Inspect features with `viva-camctl`

The `viva-camctl` CLI exposes basic feature access via `get` and `set` subcommands.  [oai_citation:0‡GitHub](https://github.com/VitalyVorobyev/genicam-rs)  

You need:

- The camera IP (from the discovery tutorial).
- Optionally, the host interface IP if you have multiple NICs.

### 1.1. Read a feature by name

Example: read `ExposureTime` from a camera at `192.168.0.10`:

```bash
cargo run -p viva-camctl -- \
  get --ip 192.168.0.10 --name ExposureTime
```

You should see:
- The current value.
- The type (e.g. Float or Integer).
- Possibly range information (min/max/increment) if available.

If you prefer machine-readable output, add --json:

```bash
cargo run -p viva-camctl -- \
  get --ip 192.168.0.10 --name ExposureTime --json
```

This is handy for scripting and CI.

### 1.2. Write a feature by name

To change a value, use the set subcommand. For example, set exposure to
5000 microseconds:

```bash
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name ExposureTime --value 5000
```

Then verify:

```bash
cargo run -p viva-camctl -- \
  get --ip 192.168.0.10 --name ExposureTime
```

If the value doesn’t change:
- The feature may be read-only (depending on acquisition state).
- There may be constraints (e.g. limited range, alignment).
- Another feature (like ExposureAuto) may be overriding manual control.

Those cases are described in more depth in the [viva-genapi chapter](../crates/viva-genapi.md).

⸻

## Step 2 – Work with selectors

Many cameras use selectors to multiplex multiple logical settings onto the
same underlying registers. A common pattern is:
- GainSelector = All, Red, Green, Blue, …
- Gain = value for the currently selected channel.

When you change GainSelector, you are effectively changing which “row” you
are editing. The NodeMap takes care of switching the right registers.

### 2.1. Inspect which selectors exist

You can use viva-camctl to dump a selector feature and see its possible values.
For example, to inspect GainSelector:

```bash
cargo run -p viva-camctl -- \
  get --ip 192.168.0.10 --name GainSelector --json
```

Look for:
- The current value (e.g. "All").
- The list of allowed values / enum entries.

### 2.2. Change a feature through a selector

To set different gains for different channels, a typical sequence is:

```bash
# Select the red channel, then set Gain
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name GainSelector --value Red

cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name Gain --value 5.0

# Select the blue channel, then set Gain
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name GainSelector --value Blue

cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name Gain --value 3.0
```

From your perspective, you are just changing features. Internally,
viva-genapi:
- Evaluates the selector.
- Resolves which nodes and registers are active.
- Applies any SwissKnife expressions as needed.

The [selectors_demo￼example](../crates/genicam.md) in the viva-genicam crate
shows this pattern in Rust.  ￼

⸻

## Step 3 – Do the same from Rust (genicam examples)

The genicam crate provides examples that mirror the CLI operations.  ￼

### 3.1. Basic get/set example

Run the get_set_feature example:

```bash
cargo run -p viva-genicam --example get_set_feature
```

This example demonstrates:
- Opening a camera (e.g. by IP or by index).
- Getting a feature by name.
- Printing its value and metadata.
- Setting a new value and verifying it.

Inspect the source under crates/viva-genicam/examples/get_set_feature.rs for a
minimal template you can reuse in your own project.

Typical pseudo-flow inside that example (simplified):

```rust
// Pseudocode sketch — see the actual example for details
let mut ctx = genicam::Context::new()?;
let cam = ctx.open_by_ip("192.168.0.10".parse()?)?;
let mut nodemap = cam.nodemap()?;

// Read a float feature
let exposure: f64 = nodemap.get_float("ExposureTime")?;
println!("ExposureTime = {} us", exposure);

// Write a new value
nodemap.set_float("ExposureTime", 5000.0)?;
```

Types and method names may differ slightly; always follow the real example in
the repository for exact signatures.

### 3.2. Selectors demo

To see selector logic in code, run:

```bash
cargo run -p viva-genicam --example selectors_demo
```

This example walks through:
- Enumerating selector values.
- Looping over them to set/read the associated feature.
- Printing out the effective values per selector.

This is a good reference if you need to build a UI that exposes per-channel
settings (e.g. separate gains per color channel).

⸻

## Step 4 – When you might need raw register access

Most applications should prefer feature-by-name access via GenApi:
- You get type safety (integers vs floats vs enums).
- You respect vendor constraints and SFNC behaviour.
- Your code is more portable across cameras.

However, there are cases where raw registers are still useful:
- Debugging unusual vendor behaviour or firmware bugs.
- Working with undocumented features that are not in the XML.
- Bringing up very early prototypes where the GenApi XML is incomplete.

The lower-level crates (viva-gige and viva-gencp) expose primitives for reading
and writing device memory directly. Refer to:
- [viva-gige chapter](../crates/viva-gige.md)
- [viva-gencp chapter](../crates/viva-gencp.md)

for details and examples. Be careful: writing to arbitrary registers can easily
put the device into an unusable state until power-cycled.

⸻

## Recap

After this tutorial you should be able to:
- Read and write GenApi features by name using viva-camctl.
- Understand and use selector features (e.g. GainSelector → Gain).
- Locate and run the genicam examples (get_set_feature, selectors_demo)
as templates for your own applications.
- Know that raw register access exists, but is usually a last resort.

Next step: [GenApi XML](./genapi-xml.md)￼— how the XML is fetched and turned
into the NodeMap that backs these features.

---
