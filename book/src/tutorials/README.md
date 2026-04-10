# Tutorials

This section walks you through typical workflows step by step.

The focus is a **GigE Vision** camera accessed over Ethernet, using:

- The `viva-camctl` CLI for quick experiments and ops work.
- The `viva-genicam` crate for Rust examples you can copy into your own code.

If you haven’t done so yet, first read:

- [Welcome](../welcome.md)
- [Quick start](../quick-start.md)

They explain how to build the workspace and verify that your toolchain works.

---

## Recommended path

If you are new to the project, the recommended reading order is:

1. [Discovery](./discovery.md)  
   Find cameras on your network, verify that discovery works, and understand
   basic NIC and firewall requirements.

2. [Registers & features](./registers.md)  
   Read and write GenApi features (e.g. `ExposureTime`), understand selectors
   such as `GainSelector`, and learn when you might need raw registers.

3. [GenApi XML](./genapi-xml.md)  
   Fetch the GenICam XML from a device, inspect it, and see how it maps to the
   NodeMap used by `viva-genapi`.

4. [Streaming](./streaming.md)  
   Start a GVSP stream, receive frames, look at stats, and learn which knobs
   matter for throughput and robustness.

You can stop after **Discovery** and **Streaming** if you only need to verify
that your camera works. The other tutorials are useful when you want to build
a full application or debug deeper GenApi issues.

---

## What you need before starting

Before running any tutorial, make sure you have:

- A working Rust toolchain (see `rust-toolchain.toml` for the pinned version).
- The workspace builds successfully:

  ```bash
  cargo build --workspace

	•	At least one GigE Vision camera reachable from your machine:
	•	Either directly connected to a NIC.
	•	Or via a switch on a dedicated subnet.

For networking details (MTU, jumbo frames, Windows specifics, etc.), see
Networking￼ once that chapter is filled in.

⸻

Tutorials overview
	•	Discovery￼
Use viva-camctl and the genicam examples to find cameras and verify that
basic communication is working.
	•	Registers & features￼
Use features by name, work with selectors, and know when to fall back to raw
register access.
	•	GenApi XML￼
Fetch XML from the device, inspect it, and understand how genapi-xml
and viva-genapi use it.
	•	Streaming￼
Start streaming, tune packet size and delay, and interpret statistics and
logging output.

Each tutorial has:
	•	A CLI variant using viva-camctl.
	•	A Rust variant using the viva-genicam crate and its examples.

---
