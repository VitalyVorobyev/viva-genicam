# ADR-0025: One GVSP Receive Engine

**Status:** Accepted
**Date:** 2026-10-11

## Context

Until 0.8 a GigE stream was assembled in two places, and they disagreed:

- **Linux and macOS** read packets only inside `FrameStream::next_block()`. Time
  the application spent on a frame was time nobody read the socket, so the
  kernel dropped packets that no counter saw. The frame timeout was a fixed
  100 ms **from the leader**, so any frame whose transfer took longer — about
  12 MB on 1 GigE, or less on a link paced by `GevSCPD` — never completed. Each
  packet was placed at `(packet id − 1) × (packet size − 36)`, assuming the
  8-byte header, so extended-ID streams (a 20-byte header, as a LUCID
  TRT009S-E sends) were written 12 bytes further off per packet. A trailer that
  overtook the last data packet dropped the frame; a repeated leader reset it.
- **Windows** used a reader thread — the path that made a JAI on a
  169.254.0.0/16 link stream 2 048 × 1 536 at 75 Mbit/s with no drops, confirmed
  on hardware by its reporter (#70, #72). But it appended packets in arrival
  order, dropped a frame at the first reordered packet, and truncated packed
  formats to `width × height × 1` byte (`bytes_per_pixel().unwrap_or(1)`).

Four more hand-written reassemblers (camctl `bench`, three examples) read the
socket themselves. `viva-gige` carried a fifth, unused (`Reassembler`,
`FrameQueue`, `BufferPool`). The counters (`StreamStats`) could not tell a
missing packet from a duplicate or a frame the camera skipped, and four of them
had no production caller at all (SR-07).

The tests for the PR that introduced this ADR reproduce each failure on the 0.8
code, through a harness that sends scripted GVSP datagrams from an ephemeral
loopback port — the old Windows path compiled on macOS for the truncation.

## Decision

**One receive engine, the same on every OS**, in `viva-genicam`'s `stream`
module:

- **Thread model.** Each `FrameStream` owns one `std::thread`. It waits up to
  **5 ms** for the socket to be readable (`poll`, `WSAPoll` on Windows) and
  then reads it without blocking; every wake-up, a datagram or the
  timeout, feeds the assembler and lets its timers fire, so blocks close on time
  whether or not the application is asking for frames. Dropping the stream sets
  a stop flag and joins the thread, within one tick. A socket error that only
  reflects an ICMP reply (`WSAECONNRESET` on Windows, `ECONNREFUSED` elsewhere)
  is transient (SR-22); any other error ends the stream, reported once.
- **Delivery.** Finished frames go into a `DeliveryQueue`: a
  `Mutex<VecDeque>` with a `Condvar` for the blocking side and a
  `tokio::sync::Notify` for the async side. **Capacity 4**; when it is full the
  oldest frame is dropped and counted as backpressure. Two front ends:
  `next_block().await` (cancel-safe, for `select!`) and
  `next_block_blocking(timeout)`, which needs no runtime and reports silence as
  `GenicamError::Timeout`.
- **Assembly** (`stream/assembly.rs`, no socket, no clock — instants are passed
  in, so every rule is unit-tested with invented time):
  - up to **3 blocks in flight**, keyed by block id; a fourth closes the oldest;
  - blocks leave **in the order they started**: a complete block waits behind
    an older open one;
  - completion needs the leader, the trailer and every data packet up to the
    one the trailer names, in any order. An early trailer only records the
    count; a repeated leader is a duplicate;
  - the **stride is `packet size − 20 − 8 − header length`**, the header length
    (8 or 20) taken from each packet;
  - the frame **buffer is allocated once** — at `PayloadSize` when the stream
    knows it (`Camera::open_stream` reads it), else from the leader's geometry
    (`ceil(width × bits / 8) + padding` per line, from the PFNC depth, capped at
    256 MiB) — and each packet is written in place. `PayloadSize` sizes the
    buffer but does not bound it: a value read before a ROI or format change
    grows rather than failing every frame, up to `max(256 MiB, PayloadSize)`.
    The buffer comes from a `BufferSource`, so a GenTL producer can hand in
    announced buffers;
  - the reusable bitmap is sized `ceil(payload / stride)`, bounded by the
    buffer whatever a trailer claims;
  - out-of-range packets are ignored and counted; a data packet longer than the
    stride is refused (written, it would overlap its neighbour), and a short one
    shown not to be the last is taken back out — its bit cleared, its bytes
    zeroed — so the frame closes `Incomplete` rather than `Complete` with shifted
    data (Codex review of #194). Its block is still counted once, as
    incomplete, and the packet in `packets_missing` as well as
    `packets_size_mismatch`; each kind is logged once with the observed size;
  - the **block timeout counts from the last packet**, default 100 ms
    (`set_block_timeout`, which replaces `set_frame_timeout`). The newest block
    does not time out while its leader is all it has.
- **Block ids** are compared wrap-aware: 16-bit ids cycle 1..=65535 skipping
  0, 64-bit ids do not wrap. A forward jump counts the skipped ids as missed
  frames; a block up to 100 ids behind the newest is late (its packets are
  ignored); further behind, or a **leader of block 1** from anything but its
  predecessor, is a restart. Only a leader starts an acquisition: a stray data
  packet or a repeat of an old block 1 is late.
- **Frames** carry `block_id`, the payload type as all 16 bits (TC-05: `0x4001`
  keeps its chunk bit) and a `FrameStatus`: `Complete`,
  `Incomplete { missing_packets }`, `DeviceStatus(u16)` or `TrailerMissing`.
  Only complete frames are delivered unless the stream opts in
  (`set_deliver_incomplete`). Other payload types parse as typed values and are
  counted as unsupported, not dropped at `trace`.
- **Counters** are atomics, written by the receive thread and the queue. Each
  block is counted once, when its fate is decided, so that
  `sent = complete + incomplete + missed + backpressure + unsupported`.
  Counters nothing can increment are gone; the resend counters arrived with
  resend (below).
  `StreamStats` lives with the engine in `viva-genicam`, and only the engine
  can record: the accumulator's recorders are crate-private.
- **One way in.** `Camera::open_stream(StreamOptions)` claims control,
  configures the stream channel (multicast included), negotiates the packet
  size, binds the socket and starts the engine with the camera's `PayloadSize`
  and time sync. Studio, `viva-service`, camctl and the Python bindings all
  use it; `StreamBuilder` plus `FrameStream::new` remain for a stream built on
  a `GigeDevice` of one's own, and `FrameStream::from_socket` for a camera
  configured elsewhere.
- **Receive buffer.** The engine requests `max(4 MiB, 2 × frame)` of
  `SO_RCVBUF`, reads back what was granted (halving Linux's doubled report), and
  warns once if it is under half, naming `net.core.rmem_max` or
  `kern.ipc.maxsockbuf`. A size the caller set explicitly is left alone.

### Resend and the source

Added in milestone S (backlog SR-04, SR-03, SR-22):

- **Only the camera is a source.** A stream opened on a camera accepts
  datagrams from the camera's own address and ignores the rest, counted in
  `packets_ignored_foreign`; `FrameStream::set_source_filter(None)` lifts it.
  The socket is bound to a port of its own (port 0 unless the caller names
  one) without `SO_REUSEPORT`, so two streams on one host no longer share a
  port and split each other's datagrams.
- **Gaps, not timers per packet.** Each block in flight keeps a list of gaps:
  a packet beyond the next one expected opens one covering every id in
  between, and an arrival inside a gap splits it (`stream/resend.rs`, no
  socket, no clock). A gap is asked for once it is 2 ms old — a packet only
  overtaken fills it first — as one request for the whole run, then every
  20 ms while it stays open, three requests at most. A block's requests,
  retries included, stop at a quarter of its packets (rounded up). The
  numbers are `FrameStream` setters. A lost trailer is never asked for:
  nothing after it says it is missing. A refused or short packet counts as
  arrived — the camera would send the same bytes again.
- **The request** is `PACKETRESEND_CMD` in the layout Wireshark dissects — 12
  bytes, or 20 with extended ids (flag `0x10`) — with no acknowledgement
  asked, sent by the receive thread **from the stream socket** to the camera's
  GVCP address. A failed send never ends the stream; a `WSAECONNRESET` from it,
  like one from a read, is not even logged above debug (SR-22).
- **When to ask.** `ResendPolicy::Auto` (the default) asks only when bit `0x4`
  of GVCP Capability (`0x0934`) is set, read once at open; a multicast stream
  and one from `FrameStream::from_socket` never ask unless told to
  (`enable_resend`) — a host that only watches must not.
- **Answers.** A camera that cannot resend says so with a bare GVSP header
  whose status names why, which the engine reads before parsing anything else
  and counts as an error-status packet. `0x800C` (`PACKET_UNAVAILABLE`) and
  `0x8012` (`PACKET_REMOVED_FROM_MEMORY`) stop retries for only the named
  requested packet; `0x8011` (`..._AND_PREVIOUS_...`) stops the named packet
  and earlier missing ids, preserving any higher tail. Only requested ids
  count as unavailable; duplicate answers do not count again; `0x8010`
  (`NOT_YET_AVAILABLE`) and `0x8014` (`TEMPORARILY_UNAVAILABLE`) leave the gap
  to its retries. Status names are Wireshark's.
- **Counters.** `resend_requested` counts each packet asked for once, however
  many requests it took; then at most once more, in `resend_recovered` if it
  arrived or `resend_unavailable` if the camera said so. A frame whose lost
  packets came back is `Complete`.

The fake camera answers `PACKETRESEND_CMD` from its own reading of the field
table: it keeps the last four blocks as built, before scripted loss, sends the
packets again from its stream socket, and answers a block it no longer holds
with one header-only `0x800C` packet per requested id, matching the recorded
Manta G-125B behavior. A mixed-outcome test refuses one id in a three-packet
range and recovers both neighbours.

Packet parsing and assembly are fuzzed (`fuzz/`, a short run per target in
CI): every byte of a GVSP datagram is remote input.

The fake camera gained the options these tests need, each written with its own
constants: extended ids, a payload-type override, `GevSCPD` pacing in timestamp
ticks with the tick frequency at bootstrap 0x093C/0x0940, a deterministic
`PacketScript` keyed by block and packet id, packed formats sized by bit depth,
and `blocks_sent()`.

### Corroboration and open questions

The GigE Vision specification is not available to us. Where a rule above is a
reading of the wire, aravis agrees with it, and that is corroboration only
(see the evidence hierarchy in `CLAUDE.md`):

- the extended header layout and its protocol overhead —
  `ArvGvspExtendedHeader`, `ARV_GVSP_PACKET_PROTOCOL_OVERHEAD(ext_ids)`; the
  LUCID capture pinned in `viva-gige`'s parser tests has the same zero bytes;
- the 100 ms default and its measurement from the last packet —
  `ARV_GV_STREAM_FRAME_RETENTION_US_DEFAULT`, `_check_frame_completion`;
- the late window of 100 block ids and the leader-only exception —
  `ARV_GV_STREAM_DISCARD_LATE_FRAME_THRESHOLD`, `_check_frame_completion`;
- the payload-type masks `0x3fff` and `0x4000`.

For resend, aravis likewise sends the request from the stream socket without
asking for an acknowledgement, checks the same capability bit, waits before a
first request (`initial-packet-timeout`) and caps requests at a share of the
frame (`packet-request-ratio`), and stops a frame's requests on `0x800C`,
`0x8011` and `0x8012`. Its standard-id request writes the block id as the whole
first word, so its channel half is always 0; ours follows Wireshark's field
table and carries the channel.

Where we depart from aravis, deliberately: frames missed are counted per
skipped id (aravis counts gap events); a return to block id 1 is a restart
for a leader (aravis treats a short backward jump as late, which would
discard the first blocks after a quick AcquisitionStop/Start); an oversized data packet is
refused (aravis writes it and truncates at the buffer end); a resend request is
retried a fixed number of times (aravis retries until the frame's ratio or its
retention runs out) and stops for the ids explicitly named by an "unavailable" answer (aravis
stops the whole frame). None of these has been checked against hardware, and neither has
anything about resend: whether cameras accept a request from a port other than
the control channel's, and how they answer one they cannot serve, are open
(backlog).

## Consequences

### Positive

- One implementation to test and fix; Windows gets reordering, extended ids and
  packed formats, Linux and macOS get a reader that never stalls on the
  application.
- A frame of any size or pacing completes; only silence ends it.
- The counters say where a frame went, and the accounting identity is a test.
- The assembler is socket-free, so the GVSP harness and unit tests never bind
  UDP 3956 and run beside the fake-camera suites.

### Negative

- A thread per stream on every OS, and a copy from the receive buffer into the
  frame buffer (the datagram itself is parsed without copying).
- Frames leave in block order, so a complete frame can wait up to one block
  timeout behind an older one that lost its trailer.
- `Frame`, `StreamStats` and `FrameStream` change shape; Viva Studio,
  `viva-service`, camctl and the Python bindings follow in the same release.
- Everything here is derived from aravis, Wireshark and arithmetic, and
  verified on the fake; the release notes say so until a reporter's hardware
  confirms it.

## Amendment — 2026-10-11 (milestone L)

- The receive thread no longer reads with a receive timeout. Microsoft
  documents a socket whose receive timed out as being in an indeterminate
  state; two Windows CI runs each lost one frame of a harness test with resend
  off, consistent with a datagram lost as the 5 ms timeout fired. It now waits
  for readiness and reads non-blocking (`viva_gige::nic::wait_readable`).
- `0x8011` closes and counts the earlier gaps (Codex review of #195), so each
  packet asked for still ends in exactly one outcome.
- A multicast stream leaves its group explicitly when it drops (SR-08), and
  `StreamOptions::monitor()` opens a receive-only stream on a group another
  application configured: nothing is claimed or written, group, port and
  packet size are read from the camera, and resend stays off.
- A frame carries the leader's `padding_x`; conversion reads each line from
  its own start when it is non-zero (`Frame::row_stride`).

**2026-10-25 amendment:** a per-packet refusal previously removed its entire
containing gap. A deterministic mixed-outcome regression showed that this
suppressed retries for recoverable neighbours and overcounted unavailability.
The gap is now split around just the refused id; an all-previous answer clips
at that id. Both remaining sides retain their request counts and due times.
An unsolicited single-packet refusal does not suppress an unrequested gap.
The public API, retry limits and request budget are unchanged. No new camera
claim is made: this is covered by pure state tests and ephemeral-loopback
receive-engine tests using the Manta’s already-recorded per-packet layout.
