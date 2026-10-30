# Streaming

Goal of this tutorial:

- Start a **GVSP** stream, from the CLI and from Rust.
- Read the statistics the stream reports — and know which of them mean
  something today.
- Understand the knobs that decide whether streaming is stable: packet size and
  MTU, packet delay, and packet resend.

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

`viva-gige` parses the packets. In `viva-genicam`, `Camera::open_stream` sets
the stream up and returns a `FrameStream`, whose receive engine reads the socket
on a thread of its own — whether or not your code is asking for a frame at that
moment — and hands you whole frames. `viva-camctl stream` is a thin CLI over
that.

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
[stream] fps=30.0 Mbps=73.73 complete=30 incomplete=0 missed=0 backpressure=0 resend=0/0/0 bytes=9216000
Summary: fps=30.0 Mbps=73.73 complete=300 incomplete=0 missed=0 backpressure=0 resend=0/0/0 bytes=92160000
```

[§4.3](#43-what-the-statistics-tell-you) says what each counter means.

If no frames arrive at all:

- Confirm nothing else is already consuming the stream (a vendor viewer holds
  the control channel exclusively).
- Confirm the host firewall permits inbound UDP on the stream port. Each
  stream gets a free port from the operating system unless you name one with
  `--port` (`destination_port` in Rust and Python), which is the way to go
  behind a firewall that admits only known ports — see
  [Networking → Letting the reply back in](../networking.md#33-letting-the-reply-back-in-firewalld).
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

NetPBM files are 8-bit, so a camera deeper than that loses its low bits in
them. `--save-format png` or `--save-format tiff` keeps every bit:

- Monochrome frames are saved grey and everything else RGB; `--rgb` makes a
  grey frame RGB too.
- A camera with more than 8 bits per sample gets a **16-bit** file: Mono10 to
  Mono16, packed or not, and Bayer mosaics, which are demosaiced at their own
  depth.
- 16-bit files hold the samples scaled to the full 16-bit range, as the PNG
  standard requires: a 12-bit `0xABC` is stored as `0xABCA`. Shift right by
  `16 − bits` to get the camera's values back. A PNG records `bits` in its
  `sBIT` chunk; a TIFF holds the same samples without it.

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --duration-s 5 --save 3 --save-format tiff
```

`stream --usb 0` takes the same `--save-format`.

Every format can be saved that the library converts: monochrome and Bayer at 8
to 16 bits, including the packed `Mono10p`/`Mono12p` and GigE Vision
`Mono10Packed`/`Mono12Packed` families; RGB and BGR, with or without alpha; and
8-bit YUV / Y'CbCr at 4:4:4, 4:2:2 and 4:1:1. A 3D or event-vision frame
cannot be converted, and saving it fails with a message saying so.

### 2.3. Event-vision cameras

Event-based (EVS) cameras send blocks of encoded events rather than images.
`viva-camctl stream` recognises the Prophesee EVT 3.0 and EVT 2.1 formats, and
handles those blocks differently from image frames:

- `--save N` writes the first N blocks as raw payloads, `block_0001.evt3` or
  `block_0001.evt21`, in the current directory. `--rgb` does not apply.
- `--raw-out <FILE>` writes **every** block's payload to one file, in arrival
  order — a continuous recording you can replay with event-vision tooling. The
  file is created, or replaced, when the first block arrives, so a stream that
  sends no event data leaves no file; `--append` adds to it instead. With
  `--raw-out`, `--save` writes no block files unless you give it a count.
  Image frames are never written there.

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --duration-s 10 --raw-out capture.evt3
```

### 2.4. Multicast

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 \
  --mode multicast --group 239.192.0.10
```

Another host can then watch the same stream without touching the camera:
see [Networking → One camera, several applications](../networking.md#61-one-camera-several-applications).

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

Setup — connect, open the stream, start acquisition:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:stream}}
```

Then the loop. Reassembly, ordering and buffering all stay inside
`FrameStream`; what you get is whole frames:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/grab_gige.rs:frame_loop}}
```

Two things to note. `open_stream` does the whole setup through the camera's own
control channel: it claims control, binds the socket, points the stream channel
at it (or at a multicast group), negotiates the packet size, reads whether the
camera can resend lost packets, and starts the receive engine with the camera's
`PayloadSize`. And `acquisition_start` /
`acquisition_stop` are ordinary synchronous calls — see
[Registers & features](./registers.md#step-3--do-the-same-from-rust).

Every frame carries its `block_id` and a `status`. By default a stream delivers
only `FrameStatus::Complete` frames; see
[§4.4](#44-incomplete-frames-and-the-block-timeout) to receive the others.
Code without an async runtime can call `next_frame_blocking(timeout)` instead of
`next_frame().await`.

Each `Frame` carries its payload and its PFNC pixel format. `Frame::to_rgb8()`
converts any format listed in [§2.2](#22-saving-frames) to 8-bit RGB, with
samples deeper than 8 bits reduced to their top eight and Bayer mosaics
demosaiced by bilinear interpolation; Viva Studio's viewer draws with the same
conversion, so it shows exactly these pixels. For the camera's own sample
values, `viva_genicam::pfnc::convert::unpack_u16` returns one `u16` per pixel,
and with the crate's `image-io` feature `viva_genicam::image_io` writes PNG and
TIFF files as `viva-camctl` does.

No camera? The [fake camera](./fake-camera.md) streams over loopback:

```bash
cargo run -p viva-genicam --example demo_fake_camera
```

### 3.1. Chunks and events

A camera can attach values to each frame — the timestamp, the exposure and
gain it used, a frame counter, the state of its I/O lines — as **chunks**, and
send **events** on the message channel when something happens: an exposure
ends, a line toggles. Which chunks and events a camera has, and how each value
is encoded, is in its own GenApi XML: the `Chunk*` and `Event*` features read
their values from the frame or the event the way any other feature reads a
register. There is no table of known chunks to fall short of.

Turn chunk mode on and enable the chunks you want:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:enable_chunks}}
```

Each frame then carries its chunks (`frame.chunks`), and `frame.payload` is the
image alone. Read every chunk feature the frame carries, or one of them:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:chunk_features}}
```

Outside `with_chunks`, a chunk feature has nothing to read and fails with
`NotAttached`: a value never outlives its frame, and nothing is read from the
camera for it.

Events work the same way. Point the message channel at the host and enable
the events you want:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:enable_events}}
```

and read an event's features from the event itself:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/demo_fake_camera.rs:event_features}}
```

The names come from your camera's XML — SFNC calls them `ChunkTimestamp`,
`ChunkExposureTime`, `EventExposureEndTimestamp` and so on — which
`viva-camctl xml --ip <CAMERA-IP> --out camera.xml` saves for you to read.
USB3 Vision cameras carry both too: chunks in the frame, events on the
camera's event interface (`viva-camctl events --usb 0`).

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
| Auto | `--auto` | `StreamOptions::auto_packet_size()` | `auto_packet_size=True` | Derived from the host NIC's MTU |
| Explicit | `--packet-size N` | `StreamOptions::packet_size(n)` | `packet_size=N` | `N`, used as a ceiling |

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

Opening a stream leaves `GevSCPD` as the camera has it. To set it, give a time:

| CLI | Rust | Python |
|---|---|---|
| `--packet-delay-ns 2000` | `StreamOptions::packet_delay(Duration::from_micros(2))` | `packet_delay_ns=2000` |

The library converts it to the camera's ticks at the frequency the camera
reports, and refuses to open the stream if the camera reports none.

### 4.3. What the statistics tell you

`FrameStream::stats()` counts every block the camera sent exactly once, when its
fate is decided:

| Counter | Meaning |
|---|---|
| `frames_complete` | Delivered to you with every packet present. |
| `frames_incomplete` | Closed with packets missing, without a trailer, or with an error status from the camera. |
| `frames_missed` | Never seen at all: gaps in the block-id sequence. |
| `frames_backpressure` | Assembled, then discarded because your code did not take them in time. The stream keeps the sixteen newest frames. |
| `resend_requested` | Lost packets asked for again ([§4.6](#46-packet-resend)), each counted once. |
| `resend_recovered` | Of those, the ones that arrived. |
| `resend_unavailable` | Of those, the ones the camera said it no longer has. |

So `frames_complete + frames_incomplete + frames_missed + frames_backpressure`
is the number of frames the camera sent; `frames_lost()` is everything but the
first. The packet counters say why: `packets_missing`, `packets_duplicate`, and
`packets_ignored_*` for what arrived and could not be used (late, out of range,
malformed, an unsupported payload type, an error status, or sent from an
address that is not the camera's). `packets_size_mismatch` above zero means
the camera and the host disagree about the packet size — retry with
`--packet-size 1500`. On the command line, `resend=` shows requested,
recovered and unavailable.

`incomplete` and `missed` point at the network or the camera; `backpressure`
points at your code. A frame whose lost packets were resent counts as complete,
so `resend_recovered` is the early warning that the network drops packets.

### 4.4. Incomplete frames and the block timeout

A block closes as incomplete when nothing has arrived for it for the **block
timeout**, 100 ms by default, counted from its *last* packet: a large frame on
a slow or paced link takes as long as it takes, and only silence ends it.
`FrameStream::set_block_timeout` changes it.

Incomplete frames are counted and, by default, not delivered.
`set_deliver_incomplete(true)` hands them over too, with a `status` that says
what happened — `Incomplete { missing_packets }`, `TrailerMissing` or
`DeviceStatus(code)` — and zeros where the lost packets' bytes belong.

### 4.5. The socket receive buffer

The receive engine asks the operating system for a receive buffer of twice the
frame size (at least 4 MiB) and reads back what it got. If the system grants
less than half, it warns once and names the limit to raise —
`net.core.rmem_max` on Linux, `kern.ipc.maxsockbuf` on macOS. Below that, a
burst the size of a frame overflows the socket and the kernel drops packets
before any counter can see them.

### 4.6. Packet resend

When a packet goes missing, the stream asks the camera to send it again, if the
camera says it can (bit `0x4` of its GVCP Capability register): a gap in a
frame's packets that is still open after 2 ms is requested, again every 20 ms
while it stays open, three times at most, and never more than a quarter of the
frame's packets in all. A frame that lost more than that is not worth flooding
the camera for; it closes incomplete. A lost trailer cannot be asked for —
nothing after it says it is missing.

Requests leave from the stream's own socket for the camera's control port
(UDP 3956), and the packets come back on the stream. A firewall that admits
the stream needs nothing more.

| | CLI | Rust | Python |
|---|---|---|---|
| Turn it off | `--no-resend` | `StreamOptions::resend(ResendPolicy::Off)` | `resend=False` |

`FrameStream::set_resend_delay`, `set_resend_retry_interval`,
`set_resend_max_requests` and `set_resend_budget` change the numbers above.
A multicast stream never asks, nor does one built with
`FrameStream::from_socket` on a camera another application controls; only
the controlling application should.

Resend has been exercised on the Allied Vision Manta G-125B; other vendors and
stream channels still need hardware coverage (see [tested cameras](../tested-cameras.md)).
If a camera of yours misbehaves with it on, `--no-resend` is the workaround, and
[the report bundle](../reporting.md) is how we fix it.

---

### 4.7. Frame rate and processing overhead

Use a release build when measuring throughput. A smaller ROI and shorter
exposure can raise the camera's frame-rate limit; read that limit after changing
geometry, and verify the resulting rate with `viva-camctl bench`. Mono8 avoids
colour conversion when grayscale is sufficient. Binning changes the sampling
and does not necessarily increase FPS.

Keep taking frames promptly and move expensive processing or saving off the
acquisition loop. The receive thread continues independently, but its delivery
queue holds up to sixteen completed frames: if your application falls behind,
the oldest waiting frame is dropped and counted as `backpressure`. Packet resend cannot recover
an application-side queue eviction. Compare complete frames and loss counters
alongside FPS.

The queue absorbs short startup or scheduling pauses; it cannot compensate for
a consumer that is consistently slower than the camera. It can retain up to
16 frame payloads, in addition to assembly buffers and frames your code holds.
A larger pending queue also permits older frames to wait longer; keep reading
promptly when image freshness matters. Frame allocation and payload copying
are unchanged.

---

## 5. Troubleshooting

**Frames go missing immediately** (`incomplete` or `missed` climbing). Check
MTU and packet size alignment first, then
lower the frame rate or ROI to confirm it is a bandwidth problem rather than a
configuration one. A dedicated NIC and switch removes a whole class of cause.

**Discovery and feature access work, but no frames arrive.** Almost always the
stream port: check the firewall for inbound UDP on it (see
[2.1](#21-a-basic-stream)). A warning that datagrams arrive only from an
address that is not the camera's means something else is sending to the
stream's port; the stream ignores it.
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
- Read the chunk values a frame carries and the features of an event.
- Tell from the counters whether frames are lost on the network (`incomplete`,
  `missed`) or in your own code (`backpressure`).
- Know which knob to reach for first: packet size and MTU, then packet delay;
  and read `resend` recoveries as a network that drops packets.

Next: the [Networking Guide](../networking.md) for host and switch
configuration, or the [viva-gige chapter](../crates/viva-gige.md) for how the
packets are actually handled.
