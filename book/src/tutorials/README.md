# Tutorials

This section walks you through typical workflows step by step.

The focus is a **GigE Vision** camera accessed over Ethernet, using:

- The `viva-camctl` CLI for quick experiments and ops work.
- The `viva-genicam` crate for Rust examples you can copy into your own code.

Each tutorial has a CLI variant and a Rust variant built on the examples in
`crates/viva-genicam/examples/`.

If you haven't yet, start with the [Quick start](../quick-start.md): it
installs the library and the CLI and runs a first discovery.

---

## Recommended path

1. [Discovery](./discovery.md) — find cameras on your network, verify that
   discovery works, and understand basic NIC and firewall requirements.
2. [Registers & features](./registers.md) — read and write GenApi features
   (e.g. `ExposureTime`), use selectors such as `GainSelector`, and learn when
   you might need raw registers.
3. [GenApi XML](./genapi-xml.md) — fetch the GenICam XML from a device, inspect
   it, and see how it maps to the NodeMap used by `viva-genapi`.
4. [Streaming](./streaming.md) — start a GVSP stream, receive frames, read the
   statistics, and learn which knobs matter for throughput and robustness.
5. [Testing without hardware](./fake-camera.md) — run everything above against
   an in-process fake camera, for tests, demos, or before your camera arrives.

You can stop after **Discovery** and **Streaming** if you only need to verify
that your camera works. The other tutorials are useful when you want to build a
full application or debug deeper GenApi issues.

---

## What you need before starting

- A Rust toolchain, 1.88 or newer, and a clone of the repository. The
  commands run from its root, as `cargo run -p viva-camctl -- …` and
  `cargo run -p viva-genicam --example …`:

  ```bash
  git clone https://github.com/VitalyVorobyev/viva-genicam
  cd viva-genicam
  ```

  With `viva-camctl` installed ([Quick start](../quick-start.md)),
  `viva-camctl …` does the same as `cargo run -p viva-camctl -- …`.

- At least one GigE Vision camera reachable from your machine — connected
  directly to a NIC, or through a switch on a dedicated subnet. No camera yet?
  Start with [Testing without hardware](./fake-camera.md).

For networking details (MTU, jumbo frames, firewalls, link-local addressing),
see the [Networking Guide](../networking.md).
