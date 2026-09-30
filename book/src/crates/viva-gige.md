# `viva-gige` — GigE Vision transport (GVCP/GVSP)

`viva-gige` implements the GigE Vision transport on Windows, Linux and macOS:
discovery and control over **GVCP**, image data over **GVSP**, plus interface
enumeration, event and action messages, and device-timestamp mapping.

It sits below `viva-genapi` — this crate moves bytes, the NodeMap decides which
bytes. Applications normally reach it through `viva-genicam`.

---

## Module map

| Module | Contents |
|---|---|
| `gvcp` | `discover`, `discover_on_interface`, `discover_all`, `force_ip`, `DeviceInfo`, `GigeDevice`, `GigeError` |
| `gvsp` | Packet parsing, frame reassembly, `StreamDest`, `StreamConfig`, chunk extraction |
| `nic` | `Iface` — interface enumeration and selection |
| `action` | `send_action`, `ActionParams`, `AckSummary` |
| `message` | The event/message channel |
| `stats` | `StreamStats` and its accumulator |
| `time` | `TimeSync` — device ticks to host time |

---

## Selecting the local interface

On a multi-NIC host, bind to the NIC that reaches the camera. `Iface` offers
four ways to get one, and they are easy to confuse:

```rust,ignore
use viva_gige::nic::Iface;

Iface::from_system("eth0")?;                  // by interface name
Iface::from_ipv4("192.168.0.5".parse()?)?;    // by *host* address
Iface::from_remote_ipv4("192.168.0.10".parse()?)?; // by the *camera's* address
Iface::list()?;                               // everything the library can see
```

`from_ipv4` takes an address **on this host**. `from_remote_ipv4` takes the
camera's address and probes the routing table for the interface that reaches
it — which is what you want when all you have is the camera's IP. Passing a
camera address to `from_ipv4` is an easy mistake to miss, because it works
against the loopback fake camera, where the two addresses coincide, and fails
against real hardware.

`Iface::list()` is also a diagnostic. It reports interfaces *as the library sees
them*, which is not always the set the OS shows, and an interface missing from
this list is invisible to discovery no matter what `ipconfig` or `ip addr`
says. `IfaceSelector` parses either spelling a user might type — an IPv4
address or an OS interface name — and `resolve()` turns it into an `Iface`.

---

## Discovery (GVCP)

A broadcast command, then replies collected for a timeout window. Each reply
becomes a `DeviceInfo` with IP, MAC, manufacturer, model, version, serial and
user-defined name.

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/list_cameras.rs:discover}}
```

`discover`, `discover_on_interface` and `discover_all` differ in which
interfaces they scan; the table in
[Discovery → Discover from Rust](../tutorials/discovery.md#step-2--discover-from-rust)
lists them. An interface that cannot be bound or broadcast on is skipped with a
warning rather than failing the whole call.

From the CLI:

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

---

## Control (GVCP)

`GigeDevice` owns one control channel:

```rust,ignore
use viva_gige::gvcp::{GigeDevice, GVCP_PORT};

let mut device = GigeDevice::open(SocketAddr::new(camera_ip.into(), GVCP_PORT)).await?;
device.claim_control().await?;

let value = device.read_register(0x0a00).await?;
device.write_register(0x0a00, value | 1).await?;

let bytes = device.read_mem(0x0200, 512).await?;
```

`read_register`/`write_register` are 32-bit at a 32-bit address;
`read_mem`/`write_mem` take a 64-bit address and a length, and chunk the
transfer to fit the transport.

### Control privilege and the heartbeat

`claim_control()` takes the Control Channel Privilege. A device **revokes it**
if no GVCP command arrives within `GevHeartbeatTimeout` — 3 000 ms is typical —
and GVSP image traffic does not count towards that timer. A camera can therefore
be streaming at full rate while the control channel times out, and the next
write fails with `AccessDenied`.

You do not have to manage this. `GigeRegisterIo` in `viva-genicam` owns a
keepalive: it reads the device's own `GevHeartbeatTimeout` and pings at a
quarter of it, so holding a `Camera` is enough. `heartbeat_timeout_ms()` and
`ping_control_channel()` are here for anyone driving `GigeDevice` directly.

### IP configuration

`force_ip` assigns a temporary address to a camera identified by MAC — useful
when a camera is on the wrong subnet and otherwise unreachable.
`write_persistent_ip` and `enable_persistent_ip` make it survive a power cycle.

```bash
cargo run -p viva-camctl -- set-ip --mac DE:AD:BE:EF:CA:FE --ip 192.168.1.100 --force
```

---

## Events and actions

- **Events** are device-to-host notifications on the message channel (exposure
  end, and vendor-defined ones). `set_message_destination` points the device at
  a host socket; `viva_genicam::EventStream` presents the result.
- **Actions** are host-to-many-devices: `send_action` broadcasts an action
  command so several cameras trigger together, optionally at a scheduled
  timestamp. `AckSummary` reports which devices acknowledged.

Both are vendor-variable. If you schedule actions, keep the time bases
consistent — see `TimeSync` below.

---

## Streaming (GVSP)

The receiver negotiates stream parameters on the control channel, then receives
UDP packets and reassembles frames by block ID.

Application code builds streams through `viva-genicam`, not this crate
directly:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:stream}}
```

`StreamBuilder` (in `viva_genicam::stream`) exposes `iface`, `dest`,
`target_mtu`, `auto_packet_size`, `packet_size`, `probe`, `packet_delay`,
`destination_port`, `multicast`, `rcvbuf_bytes` and `channel`. `FrameStream`
wraps the result and yields whole frames.

### Packet size and MTU

`GevSCPSPacketSize` is the size of the transmitted **IP packet**, so it must fit
the path MTU end to end. `StreamBuilder` chooses it in one of three modes:

- **Preserve** (the default) — start from the camera's current value and never
  raise it.
- **Auto** — `auto_packet_size()` starts from a size derived from the host
  interface's MTU (`target_mtu` can cap that MTU).
- **Explicit** — `packet_size(n)` starts from `n`, treated as a ceiling.

In every mode a GVSP test-packet probe can then lower the size when the path
drops it; `probe(false)` disables it.
[Streaming → Packet size and MTU](../tutorials/streaming.md#41-packet-size-and-mtu)
explains the modes, the probe and camera-side clamping in full. Two details
belong to this layer:

- `StreamBuilder::build` reads the register back through
  `GigeDevice::get_stream_packet_size` and puts the *effective* size in
  `StreamParams`, so reassembly follows the camera rather than the request. A
  device that will not answer the read-back keeps the requested value and logs
  a warning.
- On a large-MTU link the requested size must still be clamped to the IPv4
  maximum. Linux loopback reports MTU 65536, which would produce a
  65 508-byte datagram against the 65 507-byte limit — every `send_to` fails.
  An explicitly configured size above 65 535 is refused rather than truncated:
  `GevSCPSPacketSize` holds the size in 16 bits, so writing 70 000 would
  configure 4 464.

### Resend

The resend building blocks live here — `ResendPlanner`, `coalesce_missing`,
`GigeDevice::request_resend` — but they are **not wired into the receive
path**, so `resends` stays at zero; see
[Streaming → What the statistics do and do not tell you](../tutorials/streaming.md#43-what-the-statistics-do-and-do-not-tell-you).

### Chunk data

With `ChunkModeActive` set, the payload carries the image followed by chunk
blocks (`[id][reserved][length][data]`). `parse_chunks` extracts them and skips
what it does not recognise; `viva_genicam::ChunkMap` maps the known ones
(timestamp, exposure, gain) to typed values.

### Statistics

`StreamStats` carries `frames`, `bytes`, `drops`, `packets`, `avg_fps`,
`avg_mbps`, `avg_latency_ms` and the elapsed window. The resend and
backpressure counters stay at zero for the reason above.

---

## Timestamp mapping

Devices report a tick counter, not wall-clock time. `TimeSync` maintains a
linear mapping from device ticks to host `SystemTime`, calibrated by latching
the device timestamp against a host reading. Without that calibration there is
no origin to map from — so treat an uncalibrated host timestamp as absent
rather than as data.

---

## Logging

```bash
RUST_LOG=info,viva_gige=debug,viva_genicam=debug cargo run -p viva-camctl -- stream --ip 192.168.0.10
```

`viva-camctl` maps `-v` to `debug` and `-vv` to `trace` if you would rather not
set the variable. Useful targets: `viva_gige::gvcp` (binds, discovery, register
ops), `viva_gige::nic` (interface enumeration and socket binding),
`viva_gige::gvsp` (packet parsing), and `viva_genicam::stream` (stream setup,
packet-size negotiation and frame reassembly).

---

## Platform notes

**Windows.** Firewall profiles, jumbo frames and NIC power settings are
covered in [Networking → Windows](../networking.md#22-windows).

**Linux.** With firewalld, GVCP replies arrive from source port 3956 and the
GVSP port needs its own rule — see
[Letting the reply back in](../networking.md#33-letting-the-reply-back-in-firewalld).
`net.core.rmem_max` caps how far `rcvbuf_bytes` can go.

**Link-local.** GigE Vision cameras fall back to `169.254.0.0/16` when no DHCP
server answers. That works, but the host needs an address in the same range and
the firewall usually needs telling — the same networking chapter covers it.

---

## See also

- [`viva-gencp`](viva-gencp.md) — the message layer GVCP carries
- [`viva-genapi`](viva-genapi.md) — the NodeMap above this transport
- Tutorials: [Discovery](../tutorials/discovery.md),
  [Registers](../tutorials/registers.md), [Streaming](../tutorials/streaming.md)
- [Networking Guide](../networking.md) — MTU, firewalls, link-local
