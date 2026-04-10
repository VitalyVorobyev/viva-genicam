# `viva-gige` — GigE Vision Transport (GVCP/GVSP)

`viva-gige` implements GigE Vision transport primitives for **Windows, Linux, and macOS**:

- **Discovery** (GVCP) bound to a specific local interface
- **Control path** (GVCP register read/write, GenCP semantics)
- **Events/Actions** (GVCP)
- **Streaming** (GVSP) with reassembly, resend requests, MTU/packet-size negotiation, and basic stats

This chapter explains concepts, config knobs, and usage patterns—both via CLI (`viva-camctl`) and via Rust APIs.

> ⚠️ Names in the snippets reflect the crate import style `viva_gige` (Cargo package `viva-gige`). If an identifier differs in your codebase, adjust accordingly—we’ll keep this page updated as APIs stabilize.

---

## Feature matrix (current)

| Area | Capability | Notes |
|---|---|---|
| Discovery | Broadcast on chosen NIC | Bind to local IPv4; IPv6 is out of scope for GEV |
| Control | Register read/write | GVCP commands exposing GenCP-like semantics |
| Events | Event channel | Optional; device → host notifications |
| Actions | Action command | Host → device sync/trigger |
| Streaming | GVSP receive | Frame reassembly, missing-packet detection |
| Resend | GVCP resend requests | Windowed resend; vendor-dependent behavior |
| MTU | Negotiation | 1500 default; jumbo frames if NIC/network allow |
| Packet delay | Inter-packet gap | Avoids NIC/driver overrun; per-stream configurable |
| Stats | Per-stream counters | Frames, drops, resends, latency basics |

---

## Selecting the local interface

On multi-NIC hosts, **always bind** to the NIC connected to your camera network. Two common ways:

1. **By local IPv4** (recommended for scripts/CLI):
```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
````

2. **By interface name** (if your API exposes it):

```rust
use viva_gige::net::InterfaceSelector;
let sel = InterfaceSelector::ByName("Ethernet 2"); // or ByIpv4("192.168.0.5")
```

> Windows tip: run the first discovery as **Administrator** to let the firewall prompt appear and create inbound UDP rules.

---

## Discovery (GVCP)

Discovery sends a **broadcast GVCP** command and collects replies for a small window (e.g., 200–500 ms). Each reply yields a `DeviceInfo` record (IP, MAC, manufacturer, model, name, user name, serial, firmware version, etc.).

CLI:

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

Rust pattern:

```rust
use viva_gige::{discovery::discover_on, net::InterfaceSelector, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let iface = InterfaceSelector::ByIpv4("192.168.0.5".parse().unwrap());
    let devices = discover_on(iface).await?;
    for d in devices { println!("{} {} @ {}", d.manufacturer, d.model, d.ip); }
    Ok(())
}
```

**Troubleshooting**

* No devices found → check NIC, subnet, firewall, and that you chose the right interface.
* Intermittent replies → disable NIC power saving; avoid Wi‑Fi for GEV.

---

## Control path (GVCP register access)

Most GenICam features eventually map to **register reads/writes**. `viva-gige` provides helpers to open a control session and perform 8/16/32‑bit (and block) register operations.

CLI (examples):

```bash
# Read a named feature via the high-level stack
cargo run -p viva-camctl -- get --ip 192.168.0.10 --name ExposureTime

# Write a value
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000

# Low-level register read (if exposed by CLI)
cargo run -p viva-camctl -- peek --ip 192.168.0.10 --addr 0x0010_0200 --len 4
```

Rust pattern (low‑level):

```rust
use viva_gige::{control::ControlClient, net::InterfaceSelector, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let client = ControlClient::connect("192.168.0.10".parse().unwrap(),
                                        InterfaceSelector::ByIpv4("192.168.0.5".parse().unwrap())).await?;
    let val = client.read_u32(0x0010_0200).await?;
    client.write_u32(0x0010_0200, val | 0x1).await?;
    Ok(())
}
```

> Integrates with GenApi: higher layers (NodeMap) compute addressing (incl. **SwissKnife** expressions and selectors) and call into the control client.

---

## Events & Actions (GVCP)

* **Events**: device → host notifications (e.g., exposure end). Enable in the device, bind a socket, and poll/await event messages.
* **Actions**: host → many devices synchronization (same action ID). Configure device keys and send an action command with a **scheduled timestamp** if supported.

> Behavior varies by vendor; keep time bases consistent if you schedule actions.

---

## Streaming (GVSP)

A GVSP receiver must:

1. **Negotiate** stream parameters (channel, packet size, destination host/port).
2. **Receive** UDP packets and reassemble frames by **block ID**.
3. **Detect loss**, trigger **resend** via GVCP, and time out stale frames.
4. Optionally parse **chunk data** after the image payload.

CLI (basic):

```bash
# Auto-negotiate packet size and store first 2 frames
cargo run -p viva-camctl -- stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
```

Rust pattern (high‑level sketch):

```rust
use viva_gige::{stream::{StreamBuilder, ResendPolicy}, net::InterfaceSelector};

let stream = StreamBuilder::new("192.168.0.10".parse().unwrap())
    .interface(InterfaceSelector::ByIpv4("192.168.0.5".parse().unwrap()))
    .packet_size_auto(true)                 // negotiate MTU / SCPS
    .inter_packet_delay(Some(3500))         // ns or device units, per API
    .resend_policy(ResendPolicy::Windowed { max_attempts: 2, window_packets: 64 })
    .socket_rcvbuf_bytes(16 * 1024 * 1024)  // increase OS receive buffer
    .build()
    .await?;

while let Some(frame) = stream.next().await { /* reassembled frame bytes + metadata */ }
```

### Resend

* **Windowed** resend: track missing packet ranges within a frame and request them once or twice.
* **Cut‑loss threshold**: abandon a frame when late/missing packets exceed a limit to avoid backpressure.

### MTU & Packet Size

* Start with **1500 MTU**. If NIC/network support jumbo frames, negotiate **8–9 kB** packet size for fewer syscalls.
* Ensure **both camera and NIC** are configured for the desired MTU.

### Inter‑packet Delay (IPD)

* Add a small delay between packets at the source to prevent RX ring overflow on the host/NIC.
* Useful on older NICs, laptops, and Windows where socket buffers may be smaller.

### Socket buffers

* Increase `SO_RCVBUF` to 8–32 MiB on the receive socket when streaming high‑rate video.
* On Linux, `net.core.rmem_max` may cap this; on Windows/macOS, OS caps also apply.

### Chunk mode

* When **chunks** are enabled, the payload contains **image** followed by one or more **chunk blocks** (ID, length, data).
* Parsers should skip unknown chunk IDs gracefully.

### Timestamp mapping

* Many devices expose a **tick counter**. Keep a linear mapping `(tick → host time)` using the first packets of each frame or periodic sync.

---

## Configuration knobs (cheat sheet)

| Knob                       | Purpose                          | Typical value                    |
| -------------------------- | -------------------------------- | -------------------------------- |
| `--iface <IPv4>`           | Bind discovery/stream to NIC     | Your NIC IP, e.g., `192.168.0.5` |
| `--packet-size` / `--auto` | Fixed vs. negotiated packet size | `--auto` first, then pin         |
| `--ipd`                    | Inter‑packet delay               | 2000–8000 (units per API)        |
| `--rcvbuf`                 | Socket receive buffer            | 8–32 MiB                         |
| `--resend`                 | Resend policy                    | `windowed,max=2,win=64`          |
| `--save N`                 | Write first N frames             | Debugging/validation             |

> Map these to environment variables if you prefer config files for deployments.

---

## Error handling & logging

Enable logs with `RUST_LOG`/`tracing_subscriber`:

```bash
RUST_LOG=info,viva_gige=debug cargo run -p viva-camctl -- stream ...
```

Common categories:

* `discovery` (binds, broadcast, replies)
* `control` (register ops, timeouts)
* `stream` (packet loss, reorder, resends, frame stats)

---

## Windows specifics

* **Firewall**: allow inbound UDP for discovery and the chosen stream port.
* **Jumbo frames**: enable on the NIC Advanced settings (and on switches).
* **Buffering**: larger `SO_RCVBUF` helps; keep system power plan on **High performance**.

---

## Minimal end‑to‑end example

```rust
use viva_gige::{discovery::discover_on, control::ControlClient, stream::StreamBuilder, net::InterfaceSelector};

# #[tokio::main]
# async fn main() -> anyhow::Result<()> {
let iface = InterfaceSelector::ByIpv4("192.168.0.5".parse().unwrap());
let devices = discover_on(iface).await?;
let cam = devices.first().expect("no cameras");

let ctrl = ControlClient::connect(cam.ip, iface).await?;
// Example: ensure streaming is stopped before reconfiguring
// ctrl.write_u32(REGISTER_ACQ_START_STOP, 0)?; // placeholder address

let mut stream = StreamBuilder::new(cam.ip)
    .interface(iface)
    .packet_size_auto(true)
    .socket_rcvbuf_bytes(16 * 1024 * 1024)
    .build()
    .await?;

if let Some(frame) = stream.next().await { println!("got {} bytes", frame.bytes.len()); }
# Ok(()) }
```

---

## See also

* [`viva-gencp`](genicp.md): message layouts & helpers for control path
* [`viva-genapi-xml`](genapi-xml.md) and [`viva-genapi`](genapi-core.md): NodeMap, selectors, **SwissKnife** evaluation
* Tutorials: [Registers](../tutorials/registers.md), [Streaming](../tutorials/streaming.md)
