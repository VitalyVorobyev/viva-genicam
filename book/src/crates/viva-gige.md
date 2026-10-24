# `viva-gige` — GigE Vision transport (GVCP/GVSP)

`viva-gige` implements the GigE Vision transport on Windows, Linux and macOS:
discovery and control over **GVCP**, image data over **GVSP**, plus interface
enumeration, event and action messages, and device-timestamp mapping.

It sits below `viva-genapi` — this crate moves bytes, the NodeMap decides which
bytes. Applications normally reach it through `viva-genicam`.

---

## What it exports

The modules are private; everything public is at the crate root, and
`viva_gige::consts` holds the protocol's vocabulary.

| Area | Items |
|---|---|
| Discovery and control (GVCP) | `discover`, `discover_on_interface`, `discover_all`, `discover_ip`, `discover_with`, `force_ip`, `DeviceInfo`, `GigeDevice`, `GvcpCapability`, `GigeError`, `GigeStatus` |
| Streaming (GVSP) | Zero-copy packet parsing (`parse_packet_ref`, `parse_header`), `StreamDest`, `StreamConfig`, `StreamParams`, chunk extraction (`parse_chunks`), `encode_packet_resend` |
| Interfaces | `Iface`, `IfaceSelector`, `mtu`, `best_packet_size` |
| Actions and events | `send_action`, `ActionParams`, `AckSummary`; `EventSocket`, `EventPacket` |
| Time | `TimeSync` — device ticks to host time |
| `consts` | GVCP opcodes, flags, bootstrap register addresses and protocol limits |

---

## Selecting the local interface

On a multi-NIC host, bind to the NIC that reaches the camera. `Iface` offers
four ways to get one, and they are easy to confuse:

```rust,ignore
use viva_gige::Iface;

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

A broadcast command — to each interface's subnet and to `255.255.255.255` —
then replies collected for a timeout window. Each reply becomes a `DeviceInfo`
with IP, MAC, manufacturer, model, version, serial, user-defined name, subnet
and gateway; `shares_subnet_with_host()` says whether the camera can be
reached from the interface that heard it. `discover_ip` asks one address
directly, and `discover_with` takes `DiscoveryOptions`
([Networking → Using --iface and discovery quirks](../networking.md#7-using---iface-and-discovery-quirks)).

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
use viva_gige::{GigeDevice, GVCP_PORT};

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
Dropping the transport stops the keepalive and waits for any request it has in
flight, so keep the Tokio runtime alive until the `Camera` is gone.

A request whose acknowledgement is slow — a file operation the device answers
with `PENDING_ACK` — holds the device for the whole wait, and the keepalive
cannot get in. `GigeDevice` therefore sends its own CCP reads while it waits,
with separate request IDs, and drains their replies
([ADR-0023](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0023-inflight-gvcp-heartbeats.md)).
`GigeRegisterIo` sets the probe period from the heartbeat window; a raw
`GigeDevice` opts in with `set_inflight_keepalive_period`. Each wait lasts
`set_control_timeout` (500 ms by default) unless the device asks for more.

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
  a host socket; `EventSocket` acknowledges each message as it arrives, so a
  slow reader costs the camera no retransmission, and keeps up to
  `EVENT_QUEUE_CAPACITY` events for the application, dropping the oldest past
  that; `stats()` counts what it received and dropped.
  `viva_genicam::EventStream` presents the result.
- **Actions** are host-to-many-devices: `send_action` broadcasts an action
  command so several cameras trigger together, optionally at a scheduled
  timestamp. `AckSummary` reports which devices acknowledged, and which
  answered with an error status instead (`refusals`).

Both are vendor-variable. If you schedule actions, keep the time bases
consistent — see `TimeSync` below.

---

## Streaming (GVSP)

This crate parses GVSP: `parse_packet_ref` reads one datagram without
copying it — the standard 8-byte header or the 20-byte extended-ID one, and the
payload type as all 16 bits, so `0x4001` (image with extended chunks) keeps its
chunk bit. The receive engine in `viva-genicam` assembles the blocks, on a
thread of its own per stream, with up to three blocks in flight and a timeout
counted from each block's last packet
([ADR-0025](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0025-gvsp-receive-engine.md)).

Application code opens streams through `viva-genicam`, not this crate directly:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:stream}}
```

`Camera::open_stream` takes a `StreamOptions` (`iface`, `packet_size`,
`auto_packet_size`, `multicast`, `destination_port`, `packet_delay`, `resend`,
`rcvbuf_bytes`). The lower-level `StreamBuilder` (in `viva_genicam::stream`)
adds `dest`, `target_mtu`, `probe` and `channel`, for a stream built on a
`GigeDevice` of your own; `FrameStream::new` then starts the receive engine on
it.

Either way the socket is bound first, to a port the operating system picks
unless `destination_port` names one, and that port is what the stream channel
is pointed at; `GevSCPD` is written only for a `packet_delay`, in the camera's
timestamp ticks; a channel at or beyond `GevNumberOfStreamChannels` is refused
before anything is written; and the stream accepts datagrams only from the
camera's address.

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

`encode_packet_resend` builds `PACKETRESEND_CMD` (`0x0040`): 12 bytes of
stream channel, 16-bit block id and 24-bit first and last packet ids, or 20
bytes with extended ids (flag `0x10`): channel, a reserved half-word, 32-bit
packet ids and the 64-bit block id. It asks for no acknowledgement — the answer
is the packets themselves. The receive engine sends it from the stream socket
to the camera's control port, when `GigeDevice::gvcp_capability` has bit `0x4`
set; `GigeStatus` names the statuses a camera answers with when it cannot
(`PACKET_UNAVAILABLE` and the rest of GVCP's own range), and `parse_header`
reads such an answer, which is a header and nothing
else. When and how often the engine asks is in
[Streaming → Packet resend](../tutorials/streaming.md#46-packet-resend).

### Chunk data

With `ChunkModeActive` set, a frame's payload is a run of chunks: the image
first, then one chunk per enabled value, each its data followed by an 8-byte
tag — chunk ID and length, 32-bit big-endian. `parse_chunks` reads them from
the end of the payload and reports a malformed layout with the offset where it
went wrong. This crate does not decode the values: `viva-genicam` splits each
frame into its image and its chunks, and the camera's XML says what each chunk
holds — see [Chunks and events](../tutorials/streaming.md#31-chunks-and-events).

### Statistics

A stream's counters are `viva_genicam::StreamStats`: every block counted once as
complete, incomplete, missed (a block-id gap) or dropped to backpressure, plus
what became of the packets — see
[Streaming → What the statistics tell you](../tutorials/streaming.md#43-what-the-statistics-tell-you).

---

## Timestamp mapping

Devices report a tick counter, not wall-clock time. `TimeSync` maintains a
linear mapping from device ticks to host `SystemTime`, calibrated by latching
the device timestamp against a host reading. Without that calibration there is
no origin to map from — so treat an uncalibrated host timestamp as absent
rather than as data. Given the device's tick frequency, the mapping's slope is
that frequency and only the offset is fitted; the device's clock still drifts
from the host's (an Allied Vision Manta G-125B's by 31 ms in 20 minutes), so
calibrate again to stay within a few milliseconds over a long run.

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
`net.core.rmem_max` caps the socket receive buffer; the stream warns when it
gets less than half of what it asked for.

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
