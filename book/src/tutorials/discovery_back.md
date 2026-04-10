# Tutorial: Discover devices (GigE Vision)

In this tutorial, you’ll broadcast a GVCP discovery packet on a specific interface and list the cameras that respond.

## 1) Choose your interface
Identify the IPv4 address of your NIC that’s connected to the camera network (e.g., `192.168.0.5`). On multi‑NIC hosts, discovery should be scoped to the correct interface.

## 2) Run discovery
### Option A: Example (genicam crate)
```bash
cargo run -p viva-genicam --example list_cameras
```

The example prints each device’s key identifiers (IP/MAC, model, name). On multi‑NIC systems, export an env var or pass an argument if the example supports it (see the example’s `--help`).

### Option B: CLI (viva-camctl)

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

The `--iface` parameter binds discovery to the given local interface.

## 3) Interpret results

Typical output includes:

* Device IP/MAC, manufacturer, model, serial number
* GVCP status codes if any errors occur

## Troubleshooting

* **No devices found**

  * Verify the NIC and cable; ensure the camera and host share a subnet.
  * Check OS firewall rules for **UDP broadcast**.
  * On laptops with Wi‑Fi + Ethernet, be explicit about the interface.
* **Intermittent responses**

  * Disable power‑saving on the NIC.
  * Try standard MTU first; enable jumbo frames after things work.
* **Windows**

  * Run terminal as **Administrator** the first time; allow the firewall pop‑up.

## Where to next?

* Use the **Registers** tutorial to read and write device memory and fetch GenApi XML.
* Move on to **Streaming (GVSP)** once discovery and control work reliably.
