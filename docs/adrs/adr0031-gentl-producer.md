# ADR-0031: A GenTL Producer Over the Facade

**Status:** Accepted
**Date:** 2026-10-12

## Context

GenTL is the C interface through which machine-vision software — Halcon,
Harvesters, vendor viewers — drives any camera: the application (the
*consumer*) loads a shared library renamed `.cti` (the *producer*) and calls
its exported functions. viva-genicam had no C ABI, so none of that software
could use it (backlog GT-01). [ADR-0003](adr0003-gentl-transport.md) once made
viva a GenTL *consumer*; [ADR-0011](adr0011-pure-rust-genicam-stack.md)
reversed that. This decision is the other direction: viva *as* a producer, a
thin C layer over the facade that [ADR-0030](adr0030-errors-and-public-surface.md)
froze for it.

The authority is the EMVA GenICam GenTL Standard v1.6 (free from emva.org,
specification-level evidence) and the GenTL SFNC v1.2 for the feature names
each module's XML must use. What they fix that this decision relies on:

- **Every function is exported.** §6.1: "All functions defined in this chapter
  are mandatory and must be implemented and exported in the libraries
  interface even if no implementation for a function is necessary." Some
  functions list `GC_ERR_NOT_IMPLEMENTED` among their returns for a capability
  the producer lacks (composite buffers, flows, buffer parts — §6.3.5.3,
  §6.3.5.15–22); those may answer with it.
- **Strong exception safety.** §6.1.5: "No exception objects may be thrown of
  any exported function", and on error "it must be as if the function
  encountering the error was never called". §6.1.4: "all functions must be
  thread safe".
- **Handles are opaque, closing cascades.** §6.1.3: opening a module twice in
  one process is `GC_ERR_RESOURCE_IN_USE`, and "if a Interface module is closed
  all attached Device, Data Stream and Buffer modules are also closed". §3.1:
  `GCInitLib` twice is `GC_ERR_RESOURCE_IN_USE`; `GCCloseLib` "will free all
  resources".
- **Each module has a port.** §4.1.1: "Every GenTL module except the Buffer
  module must support the Port functions", each with a GenApi XML reached
  through `GCGetPortURLInfo` and read with `GCReadPort`; §4.1.2.1 recommends the
  `local:` scheme, the XML in the module's own virtual register map. §4.1.1.1:
  the Device module has two ports, its own and the remote device's
  (`DevGetPort`). §7 and GenTL SFNC §2 list the mandatory features (`TLID`,
  `InterfaceSelector`, `DeviceAccessStatus`, `StreamBufferHandlingMode`, …).
- **Interfaces are NICs.** §2.2.2: "An Interface module represents one
  physical interface in the system. For Ethernet based transport layer
  technologies, this would be a Network Interface Card." GenTL SFNC's mandatory
  GigE Vision interface features — `GevInterfaceMACAddress`,
  `GevInterfaceSubnetIPAddress` — describe one NIC.
- **The acquisition engine.** §5: an announced pool, an input pool and an
  output queue; a buffer is the consumer's while in neither, and "the
  acquisition engine must not write data into it" once delivered (§5.1.3).
  §5.3.1: with an empty input pool "an incoming image will be lost". §5.2.1:
  the consumer sizes buffers from `STREAM_INFO_PAYLOAD_SIZE` only when
  `STREAM_INFO_DEFINES_PAYLOADSIZE` is true, and otherwise reads `PayloadSize`
  from the remote device.
- **No GenApi inside.** §2.1.2: "It is strongly recommended not to use the
  GenApi module inside a GenTL Producer implementation", because the producer
  cannot know the consumer's node map and two caches of one device disagree.

`GenTL.h` is published "under the license of the EMVA GenICam Standard
Group"; the GenICam License (v1.0, ch. 4) lets its headers be redistributed
*without modification* only, and says nothing that covers a copy in another
project's source tree. The independent consumer we test against is Harvesters
1.4.3 with EMVA's reference GenApi (`genicam` 1.6.0 on PyPI).

## Decision

**`viva-gentl` is a `cdylib` (and an `rlib` for tests) over `viva-genicam`'s
public API, GigE Vision only. Every GenTL 1.6 function is exported; 56 of the 64 do
their job and 8 answer `GC_ERR_NOT_IMPLEMENTED`.**

### 1. Functions

| Group | Implemented | `GC_ERR_NOT_IMPLEMENTED` |
|---|---|---|
| Library | `GCGetInfo`, `GCGetLastError`, `GCInitLib`, `GCCloseLib` | |
| System | `TLOpen`, `TLClose`, `TLGetInfo`, `TLGetNumInterfaces`, `TLGetInterfaceID`, `TLGetInterfaceInfo`, `TLOpenInterface`, `TLUpdateInterfaceList` | |
| Interface | `IFClose`, `IFGetInfo`, `IFGetNumDevices`, `IFGetDeviceID`, `IFUpdateDeviceList`, `IFGetDeviceInfo`, `IFOpenDevice`, `IFGetParentTL` | |
| Device | `DevGetPort`, `DevGetNumDataStreams`, `DevGetDataStreamID`, `DevOpenDataStream`, `DevGetInfo`, `DevClose`, `DevGetParentIF` | |
| Data stream | `DSAnnounceBuffer`, `DSAllocAndAnnounceBuffer`, `DSFlushQueue` (all five operations), `DSStartAcquisition`, `DSStopAcquisition`, `DSGetInfo`, `DSGetBufferID`, `DSClose`, `DSRevokeBuffer`, `DSQueueBuffer`, `DSGetBufferInfo`, `DSGetBufferInfoStacked`, `DSGetBufferChunkData`, `DSGetParentDev` | `DSAnnounceCompositeBuffer`, `DSGetNumBufferSegments`, `DSGetBufferSegmentInfo`, `DSGetNumFlows`, `DSGetFlowInfo`, `DSGetNumBufferParts`, `DSGetBufferPartInfo`, `DSGetBufferPartInfoStacked` |
| Port | `GCReadPort`, `GCWritePort`, `GCReadPortStacked`, `GCWritePortStacked`, `GCGetPortURL`, `GCGetNumPortURLs`, `GCGetPortURLInfo`, `GCGetPortInfo` | |
| Signalling | `GCRegisterEvent`, `GCUnregisterEvent`, `EventGetData`, `EventGetDataInfo`, `EventGetInfo`, `EventFlush`, `EventKill` | |

Every info command the standard marks mandatory (M) is answered; a
conditional one (CM) is answered where the facade has the value, and is
`GC_ERR_NOT_AVAILABLE` where it does not — which §5.5 defines as "the
producer did not modify the parameter", so the consumer reads it from the
remote device (`BUFFER_INFO_XOFFSET`/`YOFFSET`, which `Frame` does not carry).
The only event type is `EVENT_NEW_BUFFER`, on a data stream; registering any
other is `GC_ERR_NOT_IMPLEMENTED`, which §4.2.1 allows ("There are no
mandatory event types"). A Buffer module has no port (§4.1.1.2).

### 2. Modules

- **System:** one per process, technology `GEV`.
- **Interface: one per host network interface with an IPv4 address,
  loopback included.** Not one for all: the standard and the GigE Vision
  interface features are per NIC, and consumers list devices per interface.
  Loopback is listed because a camera on it is a simulator (`viva-fake-gige`,
  `arv-fake-gv-camera`), which is how the producer is tested; a consumer
  filters what it shows. Its ID is the operating system's interface name;
  `TLOpenInterface` also accepts one of its IPv4 addresses (§3.2's
  convenience names). Discovery runs per interface, in `IFUpdateDeviceList`,
  for the consumer's timeout (capped at 10 s, 1 s for `GENTL_INFINITE`).
- **Device:** ID `GEV:` plus the camera's MAC; also openable by IPv4 address.
  `DEVICE_ACCESS_READONLY` opens a monitor (nothing claimed, every write
  refused), `CONTROL` claims control access, `EXCLUSIVE` claims exclusive
  access (CCP `0x1`). The access status of a camera not opened here is
  `UNKNOWN`: discovery does not say whether another host controls it.
- **Data stream:** one, `Stream0`, when the camera reports a stream channel.
- **USB3 Vision is not offered.** The facade's U3V stream still needs a Tokio
  blocking pool and USB enumeration of its own (backlog API-11); a row records
  it.

### 3. Ports and XML

Each of the System, Interface, Device and Data Stream modules serves its own
GenApi XML from its virtual register map (`local:viva_gentl_<module>.xml;…`,
schema 1.1), little-endian, with GenTL SFNC 1.2 names and the enumeration
values of the SFNC's reference XML (`TLType` `GigEVision` = 0, …). One table
per module generates both the XML and the register map, so the two cannot
disagree. Every register is `NoCache` (§7: these values "should not be cached
in the nodemap but read every time"). The remote device port is the camera's
own register map through `GigeRegisterIo`; its URLs are the camera's First
and Second URL registers, read on demand. Port accesses are byte-aligned as
§4.1 requires: the remote port widens an unaligned read to whole 32-bit words
and returns the bytes asked for.

### 4. Buffers: one copy

The receive engine assembles each block into a buffer it owns; the producer
copies the finished block once into the consumer's announced buffer (or the
one `DSAllocAndAnnounceBuffer` allocated), under the stream's lock, when it
takes a buffer from the input pool. Writing in place through the engine's
`BufferSource` seam would need a `Vec<u8>` over memory the consumer owns:
`Vec` would free it and may assume it is uniquely borrowed, which is unsound,
and the seam hands a buffer to the assembler before the block's size is known
to fit. A frame larger than its buffer is truncated and delivered with
`BUFFER_INFO_IS_INCOMPLETE` and `BUFFER_INFO_DATA_LARGER_THAN_BUFFER`; an
incomplete block is delivered with `BUFFER_INFO_IS_INCOMPLETE`, its lost bytes
zero. The buffer holds the block's whole payload — for an image with chunks,
the image chunk first and the other chunks after it, which
`DSGetBufferChunkData` locates. That needs the payload as the camera sent it,
so `Frame` gains `raw_payload` (the facade otherwise hands out only the image).

`STREAM_INFO_DEFINES_PAYLOADSIZE` is false: the producer runs no GenApi, so
`PayloadSize` is the consumer's to read from the remote device (§5.2.1), and
the engine sizes each block from its leader.

### 5. Soundness of the C ABI

- **No panic crosses the boundary.** Every export runs its body in
  `catch_unwind`; a panic becomes `GC_ERR_ERROR`, with its message in
  `GCGetLastError`. A lock poisoned by such a panic stays an error.
- **Handles are numbers, never pointers.** A handle is a key into a registry
  of open objects — distinct values that are never reused, cast to `void *`.
  A handle that is not in the registry, or names the wrong kind of object, is
  `GC_ERR_INVALID_HANDLE`; nothing is dereferenced.
- **Every pointer is checked** before use; a null one is
  `GC_ERR_INVALID_PARAMETER`, a short buffer `GC_ERR_BUFFER_TOO_SMALL` with the
  size needed, and nothing is written on an error. The remaining trust is the
  standard's own contract: a consumer's buffer is valid for the size it
  announced until it is revoked.
- **State is behind locks**, held briefly: the registry for lookups only; each
  stream its own lock and condition variable, which `EventGetData` waits on
  without holding anything else. Closing removes a whole subtree from the
  registry first, then shuts its objects down outside the lock.
- **`GCInitLib`/`GCCloseLib`** are idempotent as §3.1 says (a second call is
  `GC_ERR_RESOURCE_IN_USE` / `GC_ERR_NOT_INITIALIZED`), and closing joins every
  thread the library started, releases control of every camera and shuts the
  runtime down.
- **`GCGetLastError`** is per thread (§6.3.1.3), and the last error stays until
  the next one.

### 6. The runtime

The library owns one multi-thread Tokio runtime, created by `GCInitLib` and
shut down by `GCCloseLib` after every device has been closed. Each camera is a
`GigeRegisterIo` on it, so its keepalive runs there, and register access from
the consumer's threads blocks on it directly — the "plain thread" case of
[ADR-0027](adr0027-blocking-api-and-transport-runtimes.md). One runtime for all
cameras rather than ADR-0027's one per camera, because the library, not the
camera, is what lives as long as the consumer's session. Each acquisition
adds the receive engine's thread and one delivery thread, which moves frames
from the engine to the input pool.

With `RUST_LOG` set when `GCInitLib` runs, the producer installs a `tracing`
subscriber that writes to standard error, unless one is already installed: a
consumer is rarely a Rust program, so nothing else would make the facade's
logs visible.

The stream is opened with the facade's one recipe, which moves from
`Camera::open_stream` to `GigeRegisterIo::open_stream` so that a caller
without a node map — this producer, per §2.1.2 — can use it.

### 7. The `.cti`, and the header

`scripts/build-cti.sh` builds the `cdylib` in release mode and copies it to
`viva_gentl.cti`. `GenTL.h` is not vendored: the constants, structures and
signatures are transcribed into Rust from the standard and its header, and the
C harness downloads the header from EMVA's package in CI, checked against the
package's published SHA-256.

## Consequences

### Positive

- A GenTL consumer can use viva-genicam's GigE Vision stack — discovery,
  control and streaming — with no vendor SDK. Harvesters is the only consumer
  tested; Halcon and vendor viewers are not.
- A consumer we did not write, with EMVA's reference GenApi, tests every
  push: the producer's XMLs, the remote port and acquisition against
  `viva-fake-gige`. It already found two faults in the fake that our own
  stack had accepted: its XML was not valid GenApi (no namespace, registers
  without `<pPort>`, values with an `<Address>`), and it had no `PayloadSize`,
  which a consumer sizes its buffers by when the producer does not (§5.2.1).
  Both are fixed: the fake's XML validates against the GenApi 1.1 schema (the
  CI job checks the copy the remote port serves), and a unit test checks the
  rules it broke.
- Mapping failures to GenTL codes reads `GenApiError::Io`'s source as ADR-0030
  intended: `ACCESS_DENIED` from GVCP is `GC_ERR_ACCESS_DENIED` wherever it
  arises.

### Negative

- One copy per frame, of the whole payload.
- GigE Vision only; USB3 Vision, composite buffers, multi-part payloads,
  GenDC and every event but `EVENT_NEW_BUFFER` are not offered.
- `GevInterfaceMACAddress` and `GevInterfaceGateway` read 0: the interface
  list the transport has (`if-addrs`) carries neither.
- 32-bit targets are not built: GenTL uses `stdcall` on x86, and the release
  ships 64-bit `.cti` files only.
- Exclusive access writes CCP `0x1` alone (bit 31 in GigE Vision's
  numbering), as the GigE Vision bit definitions read; no camera has confirmed
  it, and aravis never writes it.
