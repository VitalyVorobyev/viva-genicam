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

- **Thread model.** Each `FrameStream` owns one `std::thread`. It reads a
  blocking socket with a **5 ms read timeout**; every wake-up, a datagram or the
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
    stride is refused (written, it would overlap its neighbour) and a short one
    before the last is counted, each logged once with the observed size;
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
  Counters nothing can increment are gone; resend counters arrive with resend.
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

Where we depart from aravis, deliberately: frames missed are counted per
skipped id (aravis counts gap events); a return to block id 1 is a restart
for a leader (aravis treats a short backward jump as late, which would
discard the first blocks after a quick AcquisitionStop/Start); an oversized data packet is
refused (aravis writes it and truncates at the buffer end). None of these has
been checked against hardware.

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
