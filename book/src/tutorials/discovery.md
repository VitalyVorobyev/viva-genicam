# Discovery

Goal of this tutorial:

- Verify that your host can **see** your GigE Vision camera.
- Learn how to run discovery from:
  - The `viva-camctl` CLI.
  - The `viva-genicam` Rust examples.
- Understand the most common issues (NIC selection, firewall, subnets).

If discovery does not work, the other tutorials will not help much — fix this
first.

---

## Before you begin

Make sure that:

- The workspace builds:
```bash
cargo build --workspace
```
- Your camera and host are physically connected:
- Direct cable: host NIC ↔ camera.
- Or via a switch dedicated to the camera subnet.
- The camera has a valid IPv4 address:
- From DHCP on your camera network, or
- A static address that matches the host NIC’s subnet.

For deeper network discussion (jumbo frames, tuning, etc.), see
Networking￼ once that chapter is filled in.

⸻

## Step 1 – Discover with viva-camctl

The easiest way to test discovery is the viva-camctl CLI, which wraps the
genicam crate.

### 1.1. Basic discovery

Run:

```bash
cargo run -p viva-camctl -- list
```

What to expect:
* On success, you get a table or list of devices with at least:
* IP address
* MAC address
* Model / manufacturer (if reported)
* If nothing appears:
* Check that the camera is powered and connected.
* Check that your NIC is on the same subnet as the camera.
* Check that your host firewall allows UDP broadcast on that NIC.

### 1.2. Selecting an interface explicitly

On multi-NIC systems, viva-camctl may need to be told which interface to use.

Run:

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

Where 192.168.0.5 is the IPv4 address of your host NIC on the camera
network.

If you are not sure which NIC to use:
* On Linux/macOS: use ip addr / ifconfig to inspect addresses.
* On Windows: use ipconfig and your network settings GUI.

If discovery works when `--iface` is specified but not without it, your machine
likely has multiple active interfaces and the automatic NIC choice is not what
you expect.

⸻

## Step 2 – Discover via the genicam examples

The genicam crate comes with examples that exercise the same discovery logic
from Rust code.

Run:

```bash
cargo run -p viva-genicam --example list_cameras
```

This example:
- Broadcasts on your camera network.
- Prints basic info about each device it finds.

Use this when you want to:
- See how to embed discovery into your own Rust application.
- Compare behaviour between the CLI and the library (they should match).

The code for list_cameras lives under `crates/viva-genicam/examples/` and is a
good starting point for your own experiments.

⸻

## Step 3 – Interpreting results

When discovery succeeds, you should record:
- The camera’s IP address (e.g. 192.168.0.10).
- Which host NIC / interface you used (e.g. 192.168.0.5).

You will reuse these values in later tutorials, e.g.:
- Registers & features: --ip 192.168.0.10
- Streaming: --ip 192.168.0.10 --iface 192.168.0.5

If you see multiple devices, you may want to label them (physically or in a
note) to avoid confusion later.

⸻

## Troubleshooting checklist

If viva-camctl -- list or list_cameras find no devices:
1.	Physical link
    - Is the link LED on the NIC / switch / camera lit?
    - Try a different Ethernet cable or port.
2.	Subnets
	- Host NIC and camera must be on the same subnet (e.g. both 192.168.0.x/24).
	- Avoid having two NICs on the same subnet; this can confuse routing.
3.	Firewall
	- Allow UDP broadcast on the camera NIC.
	- On Windows, make sure the executable is allowed for both “Private” and
“Public” networks or run inside a network profile that permits broadcast.
4.	Multiple NICs
	- Use --iface <host-ip> to force the correct interface.
	- Temporarily disable other NICs to confirm the problem is NIC selection.
5. Vendor tools
    - If the vendor’s viewer can see the camera but viva-camctl cannot:
    - Compare which NIC / IP the vendor tool uses.
    - Check whether the vendor tool reconfigured the camera’s IP (e.g. via
DHCP or “force IP” features).

If discovery is still failing after this checklist, capture logs with:

```bash
RUST_LOG=debug cargo run -p viva-camctl -- list --iface <host-ip>
```

and open an issue with the log output and a short description of your setup.
This will also be useful when extending the [GigE transport](../crates/viva-gige.md).
