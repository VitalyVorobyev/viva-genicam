# ADR-0026: Chunk and Event Data Through GenApi Ports

**Status:** Accepted
**Date:** 2026-10-11

## Context

A GenApi register reaches its bytes through a `<Port>`, named by `<pPort>`.
GenICam Standard v2.1.1 §2.8.16 gives a port a `<ChunkID>` when it "does not
give access to a real device, but rather to the chunk of data residing in
memory", and lets a chunk port "deal with negative addresses which will be
interpreted as offset from the back of the chunk". Ports with an `<EventID>`
are the same idea for the data an event message carries. Their addresses
count from the start of the chunk or the event, not from anywhere in the
device's memory.

Until 0.8 only `<Register>` read `<pPort>`, and refused any port but the
device. Every other register on a chunk or event port read **device memory**
at its port-relative address — 518 registers in 19 vendor documents, by the
corpus test this decision added:
`EventExposureEndTimestamp` at 0x8 returned GVCP bootstrap bytes, and a
2-byte event register at 0x2 became an unaligned `READMEM` that a Vieworks
FS-3200T refuses with `BAD_ALIGNMENT` (#112, 62 times in one Studio log).
Chunk values came instead from a hardcoded four-entry table
(`KNOWN_CHUNKS`) with 16-bit IDs, decoded little-endian from a chunk
"region" the parser looked for in the GVSP trailer, front to back. The
corpus has 32-bit chunk IDs such as FLIR's `0x0508000E`, and Baumer's HXG20
declares `ChunkImageLength` at `<Address>-4</Address>`, which the unsigned
address model could not parse.

What the evidence says about the layouts (the GigE Vision and USB3 Vision
specifications are not available to us):

- **Chunks** sit in the payload, not the trailer. Each chunk is its data
  followed by an 8-byte tag — chunk ID and data length, 32-bit, big-endian on
  GigE Vision — so a payload is read from its end. With payload type
  `0x4001` the image is the first chunk: the corpus declares an image chunk
  port (`ImagePort`, ChunkID 1, on FLIR and Point Grey; `PortChunkImage` on
  Baumer). With the chunk flag the trailer adds the chunk payload length and a
  chunk layout ID. aravis (`arv_buffer_get_chunk_data`) and Wireshark
  (`dissect_extra_chunk_info`) corroborate; neither is the authority.
- **Events**: every corpus document that names an event's timestamp puts it
  at 0x8 and the event ID at 0x2 (Basler, Baumer, FLIR, Hikrobot, JAI, Sony,
  Photonic Science), stream channel at 0x4, block id at 0x6, data at 0x10 —
  the 16-byte GigE Vision event header. The scanCONTROL 850050 (#93) puts the
  timestamp at 0x10 and data at 0x18, the 24-byte extended-ID header. An event
  port therefore addresses the event as the device sent it, header included.

## Decision

**A register's port decides where its bytes come from. Chunk and event data
is attached to the nodemap; a port with nothing attached is an error, never a
device read.**

1. **XML model.** `Addressing::Sum` carries the `<pPort>` of every
   register-backed node — `IntReg`, `MaskedIntReg`, `FloatReg`, `StringReg`,
   `Register` and each `StructEntry` of a `StructReg`. `XmlModel::ports` lists
   the `<Port>` declarations as `PortKind::Device`, `Chunk { chunk_id }` or
   `Event { event_id }`. `AddressTerm::Fixed` is an `i64`; a hex address keeps
   its bit pattern. `<pChunkID>`, and a port declaring both IDs, are recorded
   in `XmlModel::skipped`; `<SwapEndianess>` and `<CacheChunkData>` are parsed
   and not honoured, with a warning — none occurs in the corpus.
2. **Routing**, in `viva-genapi`'s `NodeMap`. The device port goes to the
   `RegisterIo` passed to each call, as before. A chunk port reads the chunk
   with its ID from the attached `ChunkData`; an event port reads the event
   attached for its ID. A `<pPort>` naming no declared port is refused unless
   it names `Device`. A plain declared port of any name is the device.
3. **Attaching** is explicit and owned by the nodemap:
   `attach_chunks(&ChunkData)` / `detach_chunks()`, and
   `attach_event(event_id, Bytes)` / `detach_events()`. `ChunkData` holds
   `bytes::Bytes` slices of the frame, so attaching copies no image data, and
   `NodeMap` stays `Send`. Each attach or detach invalidates every node that
   depends on the affected ports — registers are linked to their port in the
   dependency graph — and bumps the formula generation, so no cached value
   outlives its buffer. Attaching chunks replaces all chunks (they are one
   buffer's); attaching an event replaces only the event with that ID, as
   GenApi keeps the last event of each kind.
4. **Detached and out of range.** A chunk or event port with nothing attached
   is `GenApiError::NotAttached`, checked before any address term is
   evaluated. A negative offset counts from the end of a chunk; on an event
   port, and past either end, it is `PortOutOfRange`, naming the offset.
   `NodeMap::feature_state` reports a detached chunk or event feature as
   unavailable with no value, not as an error, so a viewer polling between
   frames is quiet. `register_address` refuses a port-relative register: its
   address is not a device address.
5. **Writes** to chunk and event ports are `PortNotWritable`, attached or
   not, before anything reaches the device.
6. **Chunk layout** (`viva-gige`): `parse_chunks` scans backwards with 32-bit
   IDs and returns borrowed chunks in payload order; a tag that claims more
   than precedes it, or a front too short for a tag, is a `ChunkError` naming
   the offset. A zero-length chunk is a chunk; the scan cannot loop. The
   trailer parser reports the chunk payload length and layout ID instead of a
   chunk region. A fuzz target covers it.
7. **Frames** (`viva-genicam`): a complete block whose payload type carries
   chunks (`0x0004`, or the `0x4000` flag) is split — `Frame::payload` is the
   image chunk, `Frame::chunks` the chunks by ID. A layout that does not parse
   leaves the payload whole and the chunks empty, logged once per stream; an
   incomplete block is not split. The receive engine assembles chunk-data
   blocks (`0x0004`), sized from `PayloadSize` or the packets.
   `KNOWN_CHUNKS`, `ChunkKind`, `ChunkValue` and the hardcoded decoding are
   deleted: a chunk value is whatever the camera's XML says it is.
8. **Facade.** `Camera::with_chunks(&frame, |cam| …)` and
   `Camera::with_event(&event, |cam| …)` attach, run the closure and detach;
   `chunk_features(&frame)` and `event_features(&event)` read every category
   feature fed by the ports the frame or event attaches
   (`NodeMap::port_features`). `Event` carries the stream channel, block id
   and the whole event as sent (`raw`), which is what attaches.

The scoped closure (`with_chunks`) was considered as the only form. It keeps
the attachment's lifetime visible, but threading borrowed data through every
`NodeMap` accessor would touch the whole evaluator, and a GenTL producer
(milestone T) needs GenApi's attach/detach model anyway. The nodemap owns the
attachment; the facade's scoped forms are the convenient path.

## Consequences

### Positive

- No register on a chunk or event port reaches device memory; the corpus test
  checks every one — 235 on chunk ports and 294 on event ports, in 21
  documents including aravis's synthetic one — detached and attached.
- Chunk and event features read the way the camera's XML declares — offset,
  length, byte order, bitfield, formula — for every camera, with no table to
  maintain.
- Baumer's `ChunkImageLength` builds; `EXPECTED_SKIPS` is empty in both corpus
  tests.
- #112's `BAD_ALIGNMENT` pattern — 2-byte event registers at 0x2 sent to the
  device — cannot occur.

### Negative

- Breaking: `ChunkKind`, `ChunkValue`, `ChunkMap`, `KNOWN_CHUNKS`,
  `parse_chunk_bytes` and `Frame::chunk` are gone; `Frame::chunks` is a
  `ChunkData`; `ChunkRaw` borrows the payload, its `id` is a `u32`, and
  `parse_chunks` returns a `Result`; `GvspTrailer` lost its chunk region;
  `AddressTerm::Fixed` is signed; `RegisterDecl::port` moved into
  `Addressing`. `viva-genapi` depends on `bytes`.
- For an image with chunks, `Frame::payload` is now the image alone.
- Nothing here has met a camera. The backward layout, the image-first chunk,
  the trailer fields and the event header offsets come from the corpus,
  aravis and Wireshark; the negative-address reading of Baumer's
  `ChunkImageLength` from the standard's text alone. TC-21 stays open until
  #112's reporter confirms.
- USB3 Vision chunks (the same layout, little-endian tags, per aravis) and
  events are not wired (API-11, TC-27). *Amended by
  [ADR-0032](adr0032-usb3-vision-transport.md):* both are wired, and one
  camera confirmed the little-endian tags and its event ports' offsets.
