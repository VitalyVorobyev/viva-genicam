# Networking

This chapter is a practical **GigE Vision networking cookbook**.

It focuses on:

- Typical **topologies** (direct cable vs switch, single vs multi-camera).
- **NIC and IP configuration** on Windows, Linux, and macOS.
- **MTU / jumbo frames** and **packet delay** basics.
- Common **pitfalls and troubleshooting**.

It is not a replacement for vendor or A3 documentation, but gives you enough
background to make `viva-camctl` and the `viva-genicam` examples work reliably.  [oai_citation:0‡Wikipedia](https://en.wikipedia.org/wiki/GigE_Vision?utm_source=chatgpt.com)  

If you have not yet done so, first go through:

- [Discovery](./tutorials/discovery.md)
- [Streaming](./tutorials/streaming.md)

They show the CLI and Rust-side pieces that depend on a working network setup.

---

## 1. Typical topologies

### 1.1. Single camera, direct connection

The simplest and most robust setup:

```text
[Camera]  <── Ethernet cable ──>  [Host NIC]
```

Characteristics:
- One camera, one host, one NIC.
- No other traffic on that link.
- Easy to reason about MTU and packet delay.

Recommended when:
- You’re bringing up a new camera.
- You’re debugging issues and want to remove variables.

### 1.2. One or more cameras through a switch

Common in real systems:

```text
[Cam A] ──\
           \
[Cam B] ────[Switch]──[Host NIC]
           /
[Cam C] ─/
```

Characteristics:
- Multiple cameras share the link to the host.
- Switch must handle the aggregate throughput.
- Switch configuration (buffer sizes, jumbo frames, spanning tree) matters.  ￼

Recommended when:
- You need more than one camera.
- You need long cable runs or multi-drop layouts.

### 1.3. Host with multiple NICs

For high throughput or separation from office traffic:

```text
[Cam network]  <── NIC #1 ──>  [Host]  <── NIC #2 ──>  [Office / internet]
```

Characteristics:
- Camera traffic isolated from general network.
- Easier to tune MTU, QoS, and firewall rules.
- In discovery and streaming, you may need to specify --iface <host-ip>.

Recommended for:
- High data rates.
- Multi-camera setups.
- Systems that must not be disturbed by office network traffic.

⸻

## 2. IP addressing basics

GigE Vision uses standard IPv4 + UDP. Each device needs a valid IPv4 address; the
host and camera(s) must share a subnet.  ￼

### 2.1. Choose a camera subnet

Pick a private network, for example:
- 192.168.0.0/24 (addresses 192.168.0.1–192.168.0.254)
- 10.0.0.0/24

Decide on:
- One address for your host NIC (e.g. 192.168.0.5).
- One address per camera (e.g. 192.168.0.10, 192.168.0.11, …).

Make sure this subnet does not conflict with your office / internet network.

### 2.2. Windows
1.	Open Network & Internet Settings → Change adapter options.
2.	Right-click the NIC used for cameras → Properties.
3.	Select Internet Protocol Version 4 (TCP/IPv4) → Properties.
4.	Choose Use the following IP address:
	- IP address: e.g. 192.168.0.5
	- Subnet mask: 255.255.255.0
	- Gateway: leave empty (for isolated camera networks).
5.	Turn off any “energy saving” features for this NIC in the driver settings if
possible (they can introduce latency/jitter).

On first run, Windows firewall may pop up asking whether to allow the binary on
Private / Public networks. Allow it on the relevant profile so UDP broadcasts
work.

### 2.3. Linux

Use either NetworkManager or manual configuration.

Manual example:

```bash
# Assign IP and bring interface up (replace eth1 with your device)
sudo ip addr add 192.168.0.5/24 dev eth1
sudo ip link set eth1 up
```

To make this permanent, use your distro’s network configuration tools (e.g.
Netplan on Ubuntu, ifcfg files on RHEL, etc.).

### 2.4. macOS

Use System Settings → Network:
1.	Select the camera NIC (e.g. USB Ethernet).
2.	Set “Configure IPv4” to “Manually”.
3.	Enter:
	- IP address: 192.168.0.5
	- Subnet mask: 255.255.255.0
4.	Leave router/gateway empty for a dedicated camera network.

⸻

## 3. MTU and jumbo frames

MTU (Maximum Transmission Unit) determines the largest Ethernet frame size.
Standard MTU is 1500 bytes; jumbo frames extend this (e.g. 9000 bytes). For
large images, jumbo frames can significantly reduce protocol overhead and CPU
load.  ￼

### 3.1. When to care

You probably need to look at MTU when:
- Frame sizes are large (multi-megapixel).
- Frame rates are high (tens or hundreds of FPS).
- You see lots of packet drops or resends at otherwise reasonable loads.

For simple bring-up and low/moderate data rates, standard MTU=1500 usually
works.

### 3.2. Enabling jumbo frames

All components in the path must agree:
- Camera
- Switch (if present)
- Host NIC

Typical steps:
- Camera: set `GevSCPSPacketSize` or similar feature to a value below the
path MTU (e.g. 8192 for MTU 9000). You can use viva-camctl set to adjust this.
- Switch: enable jumbo frames in the management UI (name and steps vary by
vendor).
- Host NIC:
    - Windows: NIC properties → Advanced → Jumbo Packet or similar.
    - Linux: sudo ip link set dev eth1 mtu 9000
    - macOS: some drivers expose MTU setting in the network settings; others do
not support jumbo frames.

After changing MTU, confirm with:

```bash
# Linux example
ip link show eth1
```

and check that TX/RX MTU matches your expectation.

⸻

## 4. Packet delay and flow control

Some cameras allow configuring inter-packet delay or packet interval:
- Without delay:
    - Camera sends packets as fast as possible.
    - High instantaneous bursts can overwhelm NICs / switches.
- With modest delay:
    - Traffic is smoother at the cost of a small increase in latency.

If you see high packet loss or many resends at high frame rates:
1.	Try slightly increasing the inter-packet delay.
2.	Observe:
    - Does the drop/resend rate decrease?
    - Is overall throughput still sufficient?

Some vendors also expose “frame rate limits” or “burst size” options. These can
also be used to ease pressure on the network at the cost of lower peak FPS.  ￼

⸻

## 5. Multi-camera considerations

When running multiple cameras:
- Total throughput is roughly the sum of each camera’s stream.
- The **bottleneck** can be:
    - The switch’s uplink to the host.
    - The host NIC’s capacity.
    - Host CPU / memory bandwidth.

Practical tips:
- Prefer a dedicated NIC for cameras.
- For 2–4 high-speed cameras, consider:
    - Multi-port NICs.
    - Separating cameras onto different NICs if possible.
- Stagger packet timing:
    - Slightly different inter-packet delays for each camera.
    - Slightly different frame rates, where acceptable.

Monitor:
- Per-camera stats (drops, resends, throughput).
- Host CPU usage.
- Switch port statistics if your hardware exposes them.

⸻

## 6. Using --iface and discovery quirks

On systems with more than one active NIC, automatic interface selection might
pick the wrong one.
- In viva-camctl, use --iface <host-ip> to force the correct NIC.
- In Rust examples, pass the desired local address when building the context
or stream (see the genicam and viva-gige crate chapters for details).

If discovery only works when you specify --iface, but not without it:
- You likely have:
    - Multiple NICs on overlapping subnets, or
    - A default route that prefers a different interface.
- This is not unusual; be explicit for production setups.

⸻

## 7. Troubleshooting checklist

Use this checklist when things don’t work as expected.

### 7.1. Discovery fails

See also the troubleshooting section in Discovery￼.
- Check link LEDs on camera, switch, and NIC.
- Confirm IP addressing:
    - Host and camera on same subnet.
    - No conflicting IPs.
- Check firewall:
    - Allow UDP broadcast / unicast on the camera NIC.
- Temporarily:
    - Disable other NICs to simplify routing.
    - Try a direct cable instead of a switch.

### 7.2. Streaming is unstable (drops / resends)
- Check MTU vs packet size; avoid exceeding path MTU.
- For high data rates:
    - Enable jumbo frames end-to-end (camera, switch, NIC).
- Reduce stress:
    - Lower frame rate or ROI.
    - Increase inter-packet delay slightly.
- Ensure dedicated NIC and switch where possible.
- Watch host CPU; if it’s near 100%, consider:
    - Better NIC / driver.
    - Moving processing off to another thread / core.

### 7.3. Vendor tool works, genicam-rs does not
Compare:
- Which NIC / IP the vendor tool uses.
   - The camera’s configured stream destination (IP/port).
   - The vendor tool might:
- Use a different MTU / packet size.
   - Adjust inter-packet delay automatically.
   - Try to replicate those parameters with viva-camctl and the NodeMap.

⸻

8. Recap

After this chapter you should:
	•	Understand basic GigE Vision network topologies and when to use each.
	•	Be able to configure a host NIC and camera addresses on Windows, Linux, and macOS.
	•	Know when and how to enable jumbo frames and adjust packet delay.
	•	Have a structured approach to debugging discovery and streaming issues.

For protocol-level details and tuning options exposed by this project:
	•	See viva-gige￼ for transport internals.
	•	See the Streaming tutorial￼ for concrete CLI and Rust examples.

---
