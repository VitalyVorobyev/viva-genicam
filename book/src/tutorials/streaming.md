# Streaming

Goal of this tutorial:

- Start a **GVSP** stream, from the CLI and from Rust.
- Read the statistics the stream reports — and know which of them mean
  something today.
- Understand the knobs that decide whether streaming is stable: packet size and
  MTU, packet delay, and where the current implementation stops.

You should already know your camera's IP and the host NIC you reach it on
([Discovery](./discovery.md)), and be able to set features
([Registers & features](./registers.md)).

---

## 1. How GVSP streaming works

1. On the **control path** (GVCP), the host configures the stream: destination
   IP and port, packet size, and the acquisition settings themselves.
2. On `AcquisitionStart`, the camera sends GVSP packets on the **stream
   channel** — a leader, the payload packets, and a trailer per frame.
3. The host reassembles them into frames and reports statistics.

`viva-gige` owns packet handling; `viva-genicam` presents `StreamBuilder` and
`FrameStream` on top; `viva-camctl stream` is a thin CLI over that.

Note that the stream channel is UDP to a **different port** than the control
channel, and that the camera chooses when to send. A firewall that permits GVCP
and not GVSP produces the confusing case where features work perfectly and no
image ever arrives.

---

## 2. Streaming with `viva-camctl`

```bash
cargo run -p viva-camctl -- stream --help
```

### 2.1. A basic stream

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --duration-s 10
```

`--iface` names the **host** NIC, either by one of its IPv4 addresses or by its
OS name (`eth0`, or a GUID on Windows) — and it is optional: omit it and the OS
is asked which interface routes to `--ip`. Without `--duration-s` the stream
runs until Ctrl+C.

Once a second you get a progress line, and a summary at the end:

```
[stream] fps=30.0 Mbps=73.73 frames=30 drops=0 resends=0
Summary: frames=300 bytes=92160000 drops=0 resends=0 avg_fps=30.0 avg_mbps=73.73
```

If no frames arrive at all:

- Confirm nothing else is already consuming the stream (a vendor viewer holds
  the control channel exclusively).
- Confirm the host firewall permits inbound UDP on the stream port — `10040` by
  default, changeable with `--port`.
- Confirm the `--iface` interface is one the camera can actually reach.

### 2.2. Saving frames

`--save N` writes the first N frames to the current directory as
`frame_0001.pgm` (`Mono8`) or `frame_0001.ppm` (anything else, and always with
`--rgb`). Both are plain NetPBM, readable by ImageJ, GIMP, OpenCV and
`Pillow` without a decoder.

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --duration-s 5 --save 3
```

`--save` defaults to **1**, so a plain `stream` run leaves one file behind;
pass `--save 0` to write nothing.

### 2.3. Event-vision cameras

Event-based (EVS) cameras send blocks of encoded events rather than images.
`viva-camctl stream` recognises the Prophesee EVT 3.0 and EVT 2.1 formats, and
handles those blocks differently from image frames:

- `--save N` writes the first N blocks as raw payloads, `block_0001.evt3` or
  `block_0001.evt21`, in the current directory. `--rgb` does not apply.
- `--raw-out <FILE>` appends **every** block's payload to one file, in arrival
  order — a continuous recording you can replay with event-vision tooling. The
  file is opened for appending, not truncated, so delete it between runs if you
  want a fresh capture. Image frames are never written there.

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --duration-s 10 --save 0 --raw-out capture.evt3
```

### 2.4. Multicast

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 \
  --mode multicast --group 239.192.0.10
```

---

## 3. Streaming from Rust

```bash
cargo run -p viva-genicam --example grab_gige -- --iface eth0 --save 5
```

The example streams from the first camera discovery finds, on the host
interface you name with `--iface` (an IPv4 address or an OS name; required).
It saves `--save N` frames (default 1) and stops. Unless you pass
`--packet-size`, it opts into automatic packet sizing — see
[§4.1](#41-packet-size-and-mtu).

Setup — connect, build the stream, start acquisition:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:stream}}
```

Then the loop. Packet reassembly, ordering and buffering all stay inside
`FrameStream`; what you get is whole frames:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:frame_loop}}
```

Two things to note. The stream is built on a **second** `GigeDevice` rather than
the one inside `Camera`, because stream setup writes to the same device while
the camera holds it. And `acquisition_start` / `acquisition_stop` are ordinary
synchronous calls — see [Registers & features](./registers.md#step-3--do-the-same-from-rust).

No camera? The [fake camera](./fake-camera.md) streams over loopback:

```bash
cargo run -p viva-genicam --example demo_fake_camera
```

---

## 4. Tuning for stability

### 4.1. Packet size and MTU

The single most important setting. `GevSCPSPacketSize` is the size of the
transmitted IP packet, so it must fit the path MTU end to end — camera, every
switch, and the host NIC.

- **Too large**: packets are fragmented or silently dropped, and frames arrive
  incomplete.
- **Too small**: more packets per frame, more per-packet overhead, and the host
  CPU becomes the bottleneck sooner.

The usual approach is jumbo frames (MTU 9000) on a dedicated camera network,
with the packet size set just below the **path** MTU — every hop, not only the
host NIC. A NIC that advertises 16114 and a switch that only forwards ~9216-byte
frames will accept a write of 16114 at both ends and still deliver nothing
(Vieworks FS-3200T through an ipTIME PoE4002; the same camera direct to the NIC
streams up to **16114**).

**What the library does.** There are three packet-size policies:

| Policy | CLI | Rust | Python | Starting size |
|---|---|---|---|---|
| Preserve (default) | — | — | — | The camera's current `GevSCPSPacketSize`, never raised |
| Auto | `--auto` | `StreamBuilder::auto_packet_size()` | `auto_packet_size=True` | Derived from the host NIC's MTU |
| Explicit | `--packet-size N` | `StreamBuilder::packet_size(n)` | `packet_size=N` | `N`, used as a ceiling |

Auto and explicit are mutually exclusive. The default leaves a camera that is
already configured for a narrower path alone; the reasoning is in
[ADR-0021](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0021-gvsp-packet-size-policy.md).

**All three then probe the path**: the library asks the camera for a GVSP test
packet and bisects downward when the size does not arrive. The probe only
ever *lowers*, so it cannot override a size you chose — it can only refuse to
stream at one the path drops, which is undetectable from either endpoint's
registers. A device that answers no test packet keeps its size unchanged, so
cameras that do not implement the mechanism are not walked down to 1500.
`StreamBuilder::probe(false)` turns it off and makes preserve literal.

Cameras **clamp** a packet size they cannot honour, and the write succeeds when
they do — nothing on the wire distinguishes "accepted" from "accepted and
reduced". The library reads `GevSCPSPacketSize` back after writing it and
follows the effective value, logging a warning when the two differ.

If a stream produces nothing, the library says so after a few seconds and names
the likely causes rather than leaving you with `frames=0`. Two messages are
worth recognising:

- *no GVSP packet has arrived* — a firewall, a lost control privilege, a
  camera waiting for a trigger, or a **path MTU** smaller than the packet size.
  Try `--packet-size 9000` or `1500` before assuming the camera is broken.
- *packets are arriving but no frame has completed* — the two ends disagree
  about the packet size. Retry with `--packet-size 1500`.

See [Networking → MTU and jumbo frames](../networking.md#4-mtu-and-jumbo-frames)
for the host-side configuration.

### 4.2. Packet delay

Many cameras expose `GevSCPD`, an inter-packet gap in timestamp ticks. Zero
means the camera sends a frame as fast as the link allows, which is what
overruns switch buffers and NIC rings first. A modest delay trades a little
latency for a lot of stability, and is usually the first thing to try when drops
appear only at high frame rates.

### 4.3. What the statistics do and do not tell you

`drops` counts frames that arrived incomplete. That number is real, and it is
the one to watch.

**`resends` is not.** GVSP defines packet resend, and this library contains the
pieces — a resend planner, and the GVCP command to request one — but they are
not wired into the receive path. Nothing in a real stream increments that
counter, so `resends=0` means "not implemented", not "none were needed". Do not
read it as evidence that your network is healthy; read `drops` instead.

The same applies to `backpressure_drops` and the resend-range counters exposed
on `StreamStats`.

---

## 5. Troubleshooting

**Drops spike immediately.** Check MTU and packet size alignment first, then
lower the frame rate or ROI to confirm it is a bandwidth problem rather than a
configuration one. A dedicated NIC and switch removes a whole class of cause.

**Discovery and feature access work, but no frames arrive.** Almost always the
stream port: check the firewall for inbound UDP on `--port` (10040 by default).
On Linux with firewalld, this is a separate rule from the GVCP one — see
[Letting the reply back in](../networking.md#33-letting-the-reply-back-in-firewalld).

**Frames arrive with the wrong size or format.** Read `PixelFormat`, `Width` and
`Height` back from the camera rather than assuming; a Bayer format interpreted
as `Mono8` looks like a plausible grey image with a fine crosshatch.

**Intermittent hiccups under load.** Look at CPU usage and other traffic on the
same NIC. On Windows, check the power profile and NIC driver settings — see
[Networking → Windows](../networking.md#22-windows).

When in doubt, save a few frames, re-run with `-vv`, and compare against the
vendor's viewer on the same cabling. If it still makes no sense,
[send us the report bundle](../reporting.md).

---

## 6. Recap

You should now be able to:

- Start a stream from the CLI and from Rust, and save frames or event blocks.
- Read `drops` as the meaningful reliability signal, and know that `resends` is
  not one yet.
- Know which knob to reach for first: packet size and MTU, then packet delay.

Next: the [Networking Guide](../networking.md) for host and switch
configuration, or the [viva-gige chapter](../crates/viva-gige.md) for how the
packets are actually handled.
