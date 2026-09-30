# Networking

This chapter is a practical **GigE Vision networking cookbook**.

It focuses on:

- Typical **topologies** (direct cable vs switch, single vs multi-camera).
- **NIC and IP configuration** on Windows, Linux, and macOS.
- **MTU / jumbo frames** and **packet delay** basics.
- Common **pitfalls and troubleshooting**.

It is not a replacement for vendor or A3 documentation, but gives you enough
background to make `viva-camctl` and the `viva-genicam` examples work reliably.

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

- You're bringing up a new camera.
- You're debugging issues and want to remove variables.

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
- The switch must handle the aggregate throughput.
- Switch configuration (buffer sizes, jumbo frames, spanning tree) matters.

Recommended when:

- You need more than one camera.
- You need long cable runs or multi-drop layouts.

### 1.3. Host with multiple NICs

For high throughput or separation from office traffic:

```text
[Cam network]  <── NIC #1 ──>  [Host]  <── NIC #2 ──>  [Office / internet]
```

Characteristics:

- Camera traffic isolated from the general network.
- Easier to tune MTU, QoS, and firewall rules.
- In discovery and streaming, you may need to specify `--iface` (see
  [§7](#7-using---iface-and-discovery-quirks)).

Recommended for:

- High data rates.
- Multi-camera setups.
- Systems that must not be disturbed by office network traffic.

---

## 2. IP addressing basics

GigE Vision uses standard IPv4 + UDP. Each device needs a valid IPv4 address,
and the host and camera(s) must share a subnet.

### 2.1. Choose a camera subnet

Pick a private network, for example:

- `192.168.0.0/24` (addresses 192.168.0.1–192.168.0.254)
- `10.0.0.0/24`

Decide on:

- One address for your host NIC (e.g. `192.168.0.5`).
- One address per camera (e.g. `192.168.0.10`, `192.168.0.11`, …).

Make sure this subnet does not conflict with your office / internet network.

### 2.2. Windows

Give the camera NIC a static address:

1. Open Network & Internet Settings → Change adapter options.
2. Right-click the NIC used for cameras → Properties.
3. Select Internet Protocol Version 4 (TCP/IPv4) → Properties.
4. Choose "Use the following IP address":
   - IP address: e.g. `192.168.0.5`
   - Subnet mask: `255.255.255.0`
   - Gateway: leave empty (for isolated camera networks).

Then check the three things that most often stop a camera working on Windows:

- **Firewall.** On first run Windows may ask whether to allow the program on
  Private / Public networks. Allow it on the profile the camera network uses —
  if in doubt, both. Discovery replies and the GVSP stream are both inbound UDP
  (the stream arrives on port `10040` by default, or whatever you pass to
  `--port`), so a firewall that blocks them breaks discovery or streaming even
  though nothing reports an error. Running the terminal as Administrator the
  first time makes sure the prompt can appear.
- **Power saving.** Turn off "energy efficient Ethernet" and similar options in
  the NIC driver's advanced settings, and keep the power plan on high
  performance; they add latency and jitter.
- **Jumbo frames and receive buffers.** If the whole path supports them, enable
  "Jumbo Packet" in the NIC's advanced settings (see [§4](#4-mtu-and-jumbo-frames)),
  and raise the receive buffers — they default low on many desktop NICs.

### 2.3. Linux

Use either NetworkManager or manual configuration.

Manual example:

```bash
# Assign IP and bring interface up (replace eth1 with your device)
sudo ip addr add 192.168.0.5/24 dev eth1
sudo ip link set eth1 up
```

To make this permanent, use your distro's network configuration tools (e.g.
Netplan on Ubuntu, NetworkManager connection files on RHEL).

If the host runs firewalld, discovery replies and the stream need explicit
rules — see [§3.3](#33-letting-the-reply-back-in-firewalld); the same two rules
apply outside the link-local range with your own subnet in place of
`169.254.0.0/16`.

### 2.4. macOS

Use System Settings → Network:

1. Select the camera NIC (e.g. USB Ethernet).
2. Set "Configure IPv4" to "Manually".
3. Enter:
   - IP address: `192.168.0.5`
   - Subnet mask: `255.255.255.0`
4. Leave router/gateway empty for a dedicated camera network.

---

## 3. Link-local (APIPA) cameras

A GigE Vision camera with no static IP and no DHCP server on the segment falls
back to an IPv4 link-local address in `169.254.0.0/16` — what Windows calls
APIPA. This is the normal state of a camera plugged straight into a host with
nothing else configured, so it is worth knowing even if you plan to assign
static addresses later.

The library discovers such cameras on all platforms, but the **host** must hold
a link-local address of its own first, and on Linux the firewall usually has to
be told to let the reply back in.

> This section is based on a bring-up write-up by
> [@InsuJeong496](https://github.com/InsuJeong496) in
> [issue #57](https://github.com/VitalyVorobyev/viva-genicam/issues/57#issuecomment-5127958912),
> which streamed from a link-local camera on Linux with the two firewall rules
> below and no vendor driver installed.

### 3.1. What is fixed and what is yours

Recipes like this one mix values the protocol fixes with values that belong to
one particular machine. Copying an address out of someone else's guide is the
usual reason they fail.

Fixed — do not change these:

| Item | Value |
|---|---|
| IPv4 link-local network | `169.254.0.0/16` |
| Directed broadcast for that network | `169.254.255.255` |
| GVCP port on the camera | UDP `3956` |

Yours — substitute your own:

| Item | Where to get it |
|---|---|
| Host interface name | `ip -brief address` (Linux) |
| Host link-local address | Let the OS assign one, or pick an unused `169.254.x.y` |
| Camera address | Whatever `viva-camctl list` reports; it can change between sessions |
| firewalld zone | `firewall-cmd --get-active-zones` |
| GVSP destination port | `10040` unless you pass `--port` to `viva-camctl stream` |

### 3.2. Giving the host a link-local address (Linux)

Normally NetworkManager assigns one automatically when a link comes up with no
DHCP answer. If it has not, add one by hand:

```bash
# Replace enp9s0f3u3 with your camera NIC. Keep the /16 and the broadcast.
sudo ip address add 169.254.105.107/16 \
  broadcast 169.254.255.255 \
  dev enp9s0f3u3
```

Check what an interface currently has:

```bash
ip -brief address
```

Once the host has a link-local address, discovery sends its directed broadcast
to `169.254.255.255:3956`.

On Windows this step is normally automatic — an adapter with no DHCP lease
self-assigns a `169.254.x.y` address. On macOS the same is true, though see the
MTU note in [§4](#4-mtu-and-jumbo-frames): jumbo frames cannot be selected there.

### 3.3. Letting the reply back in (firewalld)

The most common symptom is that discovery finds nothing while the camera is
plainly on the link. The camera answers *from* UDP port 3956 to whatever
ephemeral port the client bound, so a default-deny inbound policy drops the ACK
and discovery simply times out.

Find the zone that owns the camera interface, then allow that source port:

```bash
firewall-cmd --get-active-zones

sudo firewall-cmd --zone=public \
  --add-rich-rule='rule family="ipv4" source address="169.254.0.0/16" source-port port="3956" protocol="udp" accept'
```

Replace `public` with your zone. Keep `169.254.0.0/16` and `3956`.

Streaming needs a second rule, because GVSP arrives on the port the host asked
the camera to send to — `10040` by default:

```bash
sudo firewall-cmd --zone=public --add-port=10040/udp
```

If you override the port, allow the one you actually use:

```bash
viva-camctl stream --ip <camera-ip> --iface <host-link-local-ip> --port <PORT>
sudo firewall-cmd --zone=<your-zone> --add-port=<PORT>/udp
```

**These are runtime rules.** They vanish on the next firewall reload or reboot
unless you repeat them with `--permanent`.

### 3.4. Checking it worked

```bash
viva-camctl --iface <host-link-local-ip> list
```

At `-v` the discovery log names the interface it sent from and the address it
heard back from:

```text
INFO sending GVCP discovery interface_name=enp9s0f3u3 local=169.254.105.107 dest=169.254.255.255:3956
INFO received GVCP response interface_name=enp9s0f3u3 src=169.254.253.222:3956
```

If the first line is missing, the host has no link-local address on that NIC
(§3.2). If the first line appears but the second never does, suspect the
firewall (§3.3).

---

## 4. MTU and jumbo frames

MTU (Maximum Transmission Unit) determines the largest Ethernet frame size.
Standard MTU is 1500 bytes; jumbo frames extend this (e.g. 9000 bytes). For
large images, jumbo frames can significantly reduce protocol overhead and CPU
load.

### 4.1. When to care

You probably need to look at MTU when:

- Frame sizes are large (multi-megapixel).
- Frame rates are high (tens or hundreds of FPS).
- You see frame drops at otherwise reasonable loads.

For simple bring-up and low/moderate data rates, standard MTU=1500 usually
works.

### 4.2. Enabling jumbo frames

All components in the path must agree:

- Camera
- Switch (if present)
- Host NIC

The host NIC MTU is **not** the path MTU. A jumbo-capable NIC behind a switch
with a smaller frame limit still lets both ends *configure* a large
`GevSCPSPacketSize`; the switch then drops the oversized GVSP datagrams and the
stream shows `frames=0`. When in doubt, cap the size with
`viva-camctl stream --packet-size 9000` (or lower) rather than trusting the NIC
alone. How the library chooses and verifies the packet size is covered in
[Streaming → Packet size and MTU](tutorials/streaming.md#41-packet-size-and-mtu).

Typical steps:

- **Camera:** set `GevSCPSPacketSize` (or the vendor's equivalent feature) to a
  value below the path MTU (e.g. 8192 for MTU 9000), with `viva-camctl set` or
  `--packet-size` on `stream`.
- **Switch:** enable jumbo frames in the management UI (name and steps vary by
  vendor). Confirm the switch's *actual* maximum frame size — "jumbo" is not
  one number.
- **Host NIC:**
  - Windows: NIC properties → Advanced → "Jumbo Packet" or similar.
  - Linux: `sudo ip link set dev eth1 mtu 9000`
  - macOS: some drivers expose an MTU setting in the network settings; others
    do not support jumbo frames.

After changing MTU, confirm it took effect:

```bash
# Linux example
ip link show eth1
```

---

## 5. Packet delay and flow control

Some cameras allow configuring an inter-packet delay (`GevSCPD`) or packet
interval:

- Without delay, the camera sends packets as fast as possible, and the bursts
  can overwhelm NICs and switches.
- With a modest delay, traffic is smoother at the cost of a small increase in
  latency.

If you see frame drops at high frame rates:

1. Try slightly increasing the inter-packet delay.
2. Check whether the drop rate decreases, and whether overall throughput is
   still sufficient.

Some vendors also expose "frame rate limits" or "burst size" options. These can
also ease pressure on the network at the cost of lower peak FPS.

---

## 6. Multi-camera considerations

When running multiple cameras, total throughput is roughly the sum of each
camera's stream, and the **bottleneck** can be:

- The switch's uplink to the host.
- The host NIC's capacity.
- Host CPU / memory bandwidth.

Practical tips:

- Prefer a dedicated NIC for cameras.
- For 2–4 high-speed cameras, consider multi-port NICs, or separating cameras
  onto different NICs.
- Stagger packet timing: slightly different inter-packet delays per camera, or
  slightly different frame rates where acceptable.

Monitor:

- Per-camera statistics: drops and throughput.
- Host CPU usage.
- Switch port statistics if your hardware exposes them.

---

## 7. Using --iface and discovery quirks

On systems with more than one active NIC, automatic interface selection might
pick the wrong one. `--iface` forces the choice, and means the same thing
everywhere: in `viva-camctl`, in `viva-service`, in the Python `iface=`
argument and in the Rust examples. It names the **host** NIC, by either:

- one of its IPv4 addresses — `--iface 192.168.0.5`, or
- its OS name — `--iface eth0`, or a GUID like
  `{6394C55F-F630-4BC7-92D2-7AC320C73D1C}` on Windows.

Use whichever you have; the address is usually easier to find, and on Windows
much easier. A value that resolves to nothing prints every interface the
library can see, which is the fastest way to learn the GUID.

If discovery only works when you specify `--iface`, you likely have multiple
NICs on overlapping subnets, or a default route that prefers a different
interface. This is not unusual; be explicit in production setups.

---

## 8. Troubleshooting checklist

### 8.1. Discovery fails

Work through the
[Discovery troubleshooting checklist](./tutorials/discovery.md#troubleshooting-checklist).
If the camera has a `169.254.x.y` address, go to
[§3 Link-local (APIPA) cameras](#3-link-local-apipa-cameras) instead — the
causes there are specific and the fixes are two commands. If none of this
helps, the camera itself is the evidence we need: see
[Reporting a camera we can't open](./reporting.md).

### 8.2. Streaming is unstable (drops)

- Check packet size against the path MTU; see [§4.2](#42-enabling-jumbo-frames).
- For high data rates, enable jumbo frames end to end (camera, switch, NIC).
- Reduce stress: lower the frame rate or ROI, or increase the inter-packet
  delay slightly.
- Use a dedicated NIC and switch where possible.
- Watch host CPU; if it is near 100%, consider a better NIC or driver, or
  moving processing to another thread or core.

### 8.3. Vendor tool works, viva-genicam does not

When the vendor's viewer sees the camera or streams from it and
`viva-camctl` does not, compare what the two tools actually do:

- **Which host NIC and IP** the vendor tool uses. Pass the same one to
  `viva-camctl` with `--iface`.
- **Whether the vendor tool changed the camera's address** — DHCP, or a
  "force IP" button. Run `viva-camctl list` again afterwards.
- **Where the camera streams to.** The vendor tool may leave the camera
  configured with its own destination IP and port; make sure `viva-camctl` uses
  a host IP and port the firewall allows, or reset the camera to defaults.
- **Packet size and inter-packet delay.** Vendor tools often tune these
  automatically. Note the `GevSCPSPacketSize` and `GevSCPD` values the vendor
  tool shows, then reproduce them with `viva-camctl stream --packet-size N` and
  `viva-camctl set`.
- **Control privilege.** Close the vendor tool first: while it holds the
  control channel, nothing else can configure the camera.

Capture logs with `-vv` (or `RUST_LOG=debug`) at the same frame rate and
resolution, and if it still makes no sense,
[send us the report bundle](./reporting.md).

---

## 9. Recap

After this chapter you should:

- Understand basic GigE Vision network topologies and when to use each.
- Be able to configure a host NIC and camera addresses on Windows, Linux, and
  macOS.
- Know when and how to enable jumbo frames and adjust packet delay.
- Have a structured approach to debugging discovery and streaming issues.

For protocol-level details and tuning options exposed by this project:

- See [`viva-gige`](./crates/viva-gige.md) for transport internals.
- See the [Streaming tutorial](./tutorials/streaming.md) for concrete CLI and
  Rust examples.
