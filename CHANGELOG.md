# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Formulas can read a variable's minimum, maximum, increment or enumeration
  entry** (backlog `GA-29`, from
  [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)). GenICam
  Standard v2.1.1 §2.8.13 lets a `<SwissKnife>`, `<IntSwissKnife>`,
  `<Converter>` or `<IntConverter>` formula qualify a `<pVariable>` with
  `.Min`, `.Max`, `.Inc`, `.Value` or `.Entry.<Name>`. #139 admitted `.` into
  formula identifiers, but nothing resolved the result: `V1.Min` with only `V1`
  declared failed to build with `UnknownVariable`, and the standard's own
  spelling, `<pVariable Name="Gain.Max">Gain</pVariable>`, built but read
  `Gain`'s *value* — so its `MidRange` example, `(Gain.Max - Gain.Min) / 2`,
  evaluated to 0. Both spellings now read the named property. A bound comes
  from `<pMin>`/`<pMax>` when declared, else `<Min>`/`<Max>`, else from the
  node's `<pValue>` target; an undeclared integer increment is 1; `.Inc` on a
  `<Float>` is an evaluation error, since floats carry no increment in this
  model. Any other extension sends the node to `NodeMap::skipped()` with the
  new `GenApiError::UnknownVariableExtension` instead of evaluating it.
  `SkNode::vars` and the converters' `vars_to`/`vars_from` are now
  `Vec<FormulaVar>`, which names the property each identifier reads. None of
  the 6 390 formula elements in our 38-document vendor corpus uses either
  spelling, so corpus results are unchanged; the
  Triton2 description that motivated this is not in the corpus, so which
  spelling it uses is not verified. `<Float>` `<pMin>`/`<pMax>` and `<Integer>`
  `<pInc>` are still not parsed, so those bounds read as undeclared (`GA-33`).

- **`viva_genicam` re-exports the stream-block types** at the crate root:
  `StreamBlock`, `GenericStreamBlock`, `EvsBlock` and `EvsFormat`, which
  [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139) added only
  under `viva_genicam::stream` and `viva_genicam::evs` (backlog `DC-05`).

- **`viva-fake-u3v` refuses a read of a write-only register and records every
  read.** A ReadMem or ReadRegister touching a register its XML declares `WO`
  is answered `ACCESS_DENIED`, as `viva-fake-gige` already did, and
  `FakeU3vTransport::reads()` / `was_read()` show which addresses a client
  asked for. Its XML gains a write-only `ActionDeviceKey` and a
  `TriggerSoftware` command whose register is `RW`, so a read of it is served
  and only the log can show it happened.

### Changed

- **`viva-service` publishes event-vision blocks on their own topic,
  `genicam/devices/{id}/evs`, and never on `image`** (backlog `DC-05`;
  [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138),
  [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)). The
  acquisition loop called `next_frame`, so a Lucid Triton2 EVS block reached
  the image topic as a 1×64000 frame under a pixel format the bridge maps to
  `Unknown`. It now reads `next_block`: an image is published exactly as
  before, and an event block goes to `evs` behind a new 24-byte
  `viva_zenoh_api::EvsHeader` — event format, a per-acquisition block counter,
  the device timestamp when present, and the payload length — followed by the
  encoded events as the camera sent them. `EvsHeader` is `#[non_exhaustive]`
  and built with `EvsHeader::new`. The key is `evs` rather than `events` so it
  cannot be mistaken for GenICam device events. **`API_VERSION` is now 4**: a
  version 3 client that subscribes only to `image` receives nothing from an
  event camera, and the version is how it can tell why. Studio does not
  display event data. Tested against `viva-fake-gige` streaming the EVT 3.0
  vendor code, not against the reporter's camera.

### Fixed

- **A one-bit unsigned `<MaskedIntReg>` could be cleared but not set** (backlog
  `GA-32`, found while writing the #135 test; not reported). The write path
  took a bitfield's signedness from `min < 0`, while the read path used
  `<Sign>`. `<Min>` is optional and defaults to `i64::MIN`, so every bitfield
  without one was encoded as signed on write: a one-bit field accepted only -1
  and 0, writing 1 failed with `ValueTooWide`, and an n-bit field refused the
  upper half of its unsigned range. Writes now use the declared `<Sign>`, as
  reads do. In our vendor corpus every one of the 9 032 unsigned
  `<MaskedIntReg>` declarations omits `<Min>`; 1 275 of them, in 31 documents,
  declare `RW` or `WO`, and 302 of those are a single bit.

- **A USB3 Vision feature snapshot reported every feature as `RW` of kind
  `Unknown` with no range, read write-only features, and read command
  registers** (backlog `SVC-02`). `U3vDeviceHandle` never overrode
  `DeviceOps::get_feature_state`, so the service's placeholder answered for
  every U3V camera, and it read each node through `get_feature` — so a
  write-only node failed its own snapshot and a command's register was read,
  the two defects 0.6.0 removed from the GigE path (`SVC-08`, `ST-21`). Both
  handles now call one function, `viva_service::device::camera_feature_state`,
  generic over the transport, so the U3V snapshot carries the real kind,
  access mode, range and enum entries and reports write-only and command
  nodes with `"value": null` without reading them. The test asserts on the
  fake device's own read log.

## [0.6.0] - 2026-09-24

Three behaviour changes need a read before upgrading, all under the headings
below. Integer registers now decode with their declared byte order and sign, so
311 little-endian `<IntReg>` declarations in our vendor corpus return different
— correct — values, and a full-width unsigned register is read as two's
complement instead of failing. A single 32-bit register now goes out as
READREG/WRITEREG, falling back to memory access per session, with
`VIVA_GIGE_FORCE_READMEM` as an escape hatch. And `viva_zenoh_api::DeviceAnnounce`
is `#[non_exhaustive]` at API version 3. Most fixes here answer reports from
real hardware (#112, #135, #136, #137, #140); the little-endian one was found
while tracing #140. None is confirmed on the reporters' hardware yet.

### Added

- **Event-vision streams from Lucid Triton2 EVS cameras**
  ([#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139), contributed by
  @richbai90; [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)).
  The first non-image payload this library receives. A Triton2 EVS (Sony IMX636)
  streams Prophesee EVT 3.0 or EVT 2.1 event data over GVSP under vendor-defined
  format codes, and `FrameStream::next_block` now returns it as a
  `StreamBlock::Evs(EvsBlock)` carrying the encoded bytes exactly as sent, beside
  `StreamBlock::Image` for ordinary frames. `next_frame` is unchanged for image
  cameras. `viva-camctl stream` handles both. `viva_pfnc::PixelFormat` names the
  two codes `EvsEvt30`/`EvsEvt21`; both are vendor-defined (bit 31 set), and
  `bytes_per_pixel` answers the size of one event word, not of a pixel. Tested
  against golden bytes from a real TRT009S-E capture. `viva-service` still
  publishes an event block on the image topic, which is `DC-05` and ours to
  settle.

### Fixed

- **Setting one bit of a write-only register read the register first, and the
  camera refused the read** ([#135](https://github.com/VitalyVorobyev/viva-genicam/issues/135),
  backlog `GA-31`). Reads of `WO` nodes were already refused before reaching the
  wire; the gap was on the write side. A bitfield write is a read-modify-write,
  and `set_integer`/`set_bool` fetched the rest of the register with an
  unguarded read whenever the raw cache was cold — so a `<StructEntry>` under a
  `<StructReg AccessMode="WO">`, which inherits `WO` and always has a bitfield,
  sent a READMEM the device rejected. That write now fails locally with
  `GenApiError::MaskedWriteUnreadable`, which names the node and the register,
  and nothing is sent. A warm cache is still used, since that is the legal
  GenICam way to keep the other bits. This is our diagnosis of a path that
  matches the report, not a confirmation of the reporter's case: the report
  names no node and carries no log.

- **`viva-camctl set` reported failure after writing a write-only feature**
  (backlog `DX-11`, found while tracing #135). It read every node back after
  writing it, and a `WO` node refuses that read — so the command failed after
  the write had succeeded, and on a real camera put a rejected request on the
  wire too. The read-back now happens only for readable nodes; a write-only one
  prints `wrote <name> = <value> (write-only node; not read back)`, and `--json`
  gives `"value": null, "write_only": true`.

- **A write-only feature was missing from `viva-service`'s feature snapshot,
  and a command's register was read for a value it does not hold**
  (backlog `SVC-08`, [#135](https://github.com/VitalyVorobyev/viva-genicam/issues/135);
  `ST-21`, [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)).
  `build_feature_state` read every node's value with `?`, so a write-only node
  returned an error instead of a `FeatureState`: the introspect queryable
  answered with an error, and the bulk queryables dropped the feature from the
  snapshot Studio asks for on connect. A command's backing register was read
  like any other node, and in the vendor corpus 213 of the 490 `<pValue>`
  targets of `<Command>` nodes are declared `RW`, so such a read would reach
  the device. Now none of these is read. A write-only node, a Command, and
  every node a command writes through are reported with their access mode and
  `"value": null`, so a client can still offer the write or the execute. Two
  new `NodeMap` methods decide from the declarations, not from node names:
  **`is_write_only()`** is true when the node or anything it takes its value
  from through `<pValue>` is declared `WO`, and **`command_targets()`**
  follows each command's `<pValue>` chain as `exec_command` writes along it.
  A node declared `RW` that delegates to a `WO` register is reported as `WO`.
  The `FeatureState::value` contract documents `null`; `API_VERSION` stays 3,
  since 3 is unreleased. #135 names no node, so this is the third path we have
  found that matches it, not a confirmation. #112's attached Studio log has
  18 `access denied for node` warnings from the bulk read, one per node: 17
  registers refusing their own read, `pAcquisitionStartReg`,
  `pTriggerSoftwareReg`, `pActionDeviceKeyReg` and the like, and
  `ActionDeviceKey`, whose read failed at `pActionDeviceKeyReg` one hop down.
  `viva-genapi` raises that text only when it refuses to read a node declared
  `WO`, so we infer all 18 were local refusals — log noise rather than wire
  traffic — and all 18 are now write-only by `is_write_only()`. We do not have
  that camera's XML, so which of them are command targets is inferred from
  their names only.

- **An eight-byte register declared `<Sign>Unsigned</Sign>` could not be read at
  all** once its top bit was set — `node <name> holds an unsigned 64-bit value
  larger than i64::MAX`. Reported on two vendors' cameras by two people: a
  Vieworks FS3200T ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112),
  62 failing timestamp reads in an attached log) and a FLIR Blackfly
  ([#140](https://github.com/VitalyVorobyev/viva-genicam/issues/140), a PTP
  offset failing whenever it went negative). The value is now reinterpreted as
  two's complement instead of refused, which is lossless at the bit level and
  round-trips through the encoder. GenApi's `IInteger` *is* an `int64` (GenICam
  v2.1.1 §2.4), so there is nowhere else to put it; see
  [ADR-0022](docs/adrs/adr0022-integer-register-decoding.md) for why this
  knowingly accepts XML the specification says cannot exist. The vendor corpus
  holds **196 such registers across 22 of its 38 documents**, overwhelmingly
  `*Timestamp*` and `Chunk*`. Neither reporter has confirmed the fix on their own
  hardware yet.

- **A plain `<IntReg>` declaring `<Endianess>LittleEndian</Endianess>` was
  decoded big-endian anyway.** `NodeDecl::Integer` had no field for byte order,
  and the parser routed the element only into the bitfield builder — which
  discards it when no `<LSB>`, `<MSB>`, `<Bit>` or `<Mask>` creates a bitfield.
  So on an *unmasked* register the declared order went nowhere. **311
  declarations across 16 of the 38 corpus documents** are of that shape,
  concentrated in Point Grey, FLIR, Basler, Hikrobot and Micro-Epsilon. Masked
  registers were never affected.

  **This was not reported; it was found while tracing the register above**, and
  it changes reads *and* writes. `i64_to_bytes` had the same defect and validates
  by round-tripping through the reader, so fixing one side alone would have
  turned every little-endian write into a range error.

- **Read this before upgrading.** Two consequences worth knowing about. A `u64`
  register above `i64::MAX` — a PTP timestamp, typically — now reads as a large
  negative number rather than an error; cast back with `as u64` if you want the
  unsigned value. And because the byte-order fix moves reads and writes together,
  an application that worked around the old behaviour on **one** side only will
  now be self-inconsistent, while one that worked around both stays correct.

- **The corpus test could not have caught either of them, and now can catch the
  first.** Its `viva-genapi` stage evaluated every node against `NullIo`, which
  answers reads with zeros — a byte swap of zero is still zero, and an unsigned
  value's top bit is never set, so the weekly `Vendor XML Corpus` workflow passed
  on 2026-08-31, 09-07 and 09-14 across a union of 448 wrong nodes. Each document
  is now evaluated **twice**: once against `NullIo` and once against a test-local
  stub returning a descending byte pattern that sets the top bit in either byte
  order, where a conversion error counts as an engine defect. That second pass
  fails on **373 nodes across 19 of the 38 documents** before this change and is
  clean after. It still asserts no *values*, so a wrong-but-plausible number
  would pass it; that is the rest of backlog `GA-11`.

- **`cargo deny` was failing on `main`, and it was not the fault of the pull
  request whose CI reported it.** A refresh of `Cargo.lock` — 153 packages,
  `zenoh` 1.9.0 → 1.10.1 — clears two findings that had appeared since the last
  lockfile touch: **RUSTSEC-2026-0285**, a rustls vulnerability where TLS 1.3
  handshake messages packed into the same record as a key-changing message were
  accepted at the wrong encryption level (fixed in rustls 0.23.45), and a yanked
  `chacha20` 0.10.1. Both arrive through zenoh's QUIC link, so no code here is
  affected, but `cargo deny check` is a hard gate and the whole repository was
  red on it — including [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139),
  where it looked like a contributor's failure.

  The four advisories `deny.toml` accepts were **re-derived rather than carried
  forward**: emptied, re-run, and kept only where they still fire. All four still
  do, all four are still zenoh transitive dependencies (`lz4_flex`, `rsa`,
  `paste`, `rustls-pemfile`), and the comment now says so against 1.10.1 instead
  of 1.9.0. The procedure is written into `deny.toml`, because an exception list
  is only defensible against the lock that actually ships.

- **`viva-service` announced every GigE camera with its device id as the serial,
  and dropped the rest of its identity** (backlog `SVC-07`,
  [#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)).
  `publish_announce` took only a device id and a model, so `serial` carried
  `cam-<mac hex>` and `name` the bare model — two cameras of one model announced
  the same `name` and `model` and differed only in a field that was wrong and
  that no client rendered. The Discovery ACK's serial and user-defined name have
  been parsed since 0.3.0 and were never plumbed past the library. The announce
  is now built from the whole `DeviceInfo`: the real serial (empty when the
  camera reports none), the IP, the MAC, the user-defined name and the
  manufacturer, with `name` the user-defined name, else the model, else the
  address. `viva-service-u3v` already sent a real serial; it now sends an empty
  one rather than `"N/A"` when the descriptor is missing, plus the manufacturer.
  The reporter has not yet confirmed this on their cameras.

### Changed

- **A single 32-bit register now goes out as READREG/WRITEREG, not
  READMEM/WRITEMEM** ([#136](https://github.com/VitalyVorobyev/viva-genicam/issues/136),
  backlog `TC-22`). The reporter saw it in Wireshark: a write to
  `GevSCPSPacketSize` left the host as `WRITEMEM_CMD`. Every `RegisterIo`
  access did, and so did seven bootstrap writes in `GigeDevice` — stream
  destination, port, packet size, test packet and delay, and the message channel
  address and port — while `get_stream_packet_size` read the same register back
  with READREG. `GigeDevice::read_register_or_mem`/`write_register_or_mem` now
  pick the command: READREG/WRITEREG when the access is exactly four bytes at a
  four-byte-aligned address below 4 GiB, READMEM/WRITEMEM otherwise. The
  READREG value is re-serialised big-endian, which is byte-for-byte what READMEM
  returned, so nothing above the transport sees a difference. An 8-byte register
  is deliberately **not** split into two READREGs: a multi-register READREG has
  no defined atomicity, so a latched 64-bit timestamp could tear silently. For
  the same reason this does not address `BAD_ALIGNMENT` on #112's event
  registers (`TC-21`).

  **A device that refuses the register commands falls back, once per session.**
  A READREG or WRITEREG answered `NOT_IMPLEMENTED`, or not answered at all,
  switches that `GigeDevice` to READMEM/WRITEMEM for the rest of the session,
  logs one `WARN` naming the address, and retries the same access so the caller
  still succeeds. A timeout latches only after the memory retry succeeds, so a
  camera that has simply gone away does not cost the session its register
  commands. `ACCESS_DENIED`, `BAD_ALIGNMENT` and every other status are real
  answers and are returned as they are. Setting **`VIVA_GIGE_FORCE_READMEM`** to
  any non-empty value other than `0` starts every session on memory access, for a
  device that answers READREG wrongly rather than refusing it;
  `GigeDevice::use_memory_access` does the same in code. The CCP claim, the
  heartbeat and the persistent-IP helpers go through the same path, so they fall
  back with everything else. `GigeDevice::read_register`/`write_register` remain,
  unconditionally READREG/WRITEREG. The `viva_gige::time::ControlChannel` impl,
  whose `read_register`/`write_register` forwarded to READMEM/WRITEMEM despite
  the name, now dispatches too.

  **Not yet seen on hardware.** The CCP claim and the heartbeat have always used
  WRITEREG and READREG, so cameras already answer those two; feature registers
  over READREG are new, and a device that mishandles them is what the fallback
  and the environment variable exist to survive.

- **`viva-fake-gige` can now disagree with the host about register access**
  (backlog `TC-23`, [ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md)).
  It counts every READREG, WRITEREG, READMEM and WRITEMEM per address
  (`FakeCamera::commands`), so a test can assert which command left the host —
  READREG and READMEM return the same bytes, so a value check cannot. It fires
  the GVSP test packet from WRITEREG as well as WRITEMEM; without that, moving
  `request_test_packet` to WRITEREG would have silently stopped the path probe.
  It refuses a read of any register its XML declares `WO` with `ACCESS_DENIED`,
  through either command, where before a READMEM of a write-only address
  returned the stored bytes; and it gains a write-only `<StructReg>` with two
  `<StructEntry>` bits at `0x200B0`, the first write-only bitfield in the fake
  and the shape our reading of #135 points at. `FakeCameraBuilder::refuse_register_commands`
  answers READREG/WRITEREG with `NOT_IMPLEMENTED` or not at all, which is the
  only way to test the fallback without hardware. `FakeCamera::peek` reads device
  state without going through GVCP, so a test can check a write landed without
  trusting our own read-back.

- **`DeviceAnnounce` carries the device's identity, and the Zenoh API version is
  3** (`viva-zenoh-api`, backlog `SVC-07`/`API-13`,
  [#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)). It gains
  `ip`, `mac`, `user_name` and `manufacturer`, all `Option<String>` with
  `#[serde(default)]`, so an announce from a version 2 service still
  deserialises — tested against a v2-shaped payload. **Breaking for Rust
  callers**: the struct is now `#[non_exhaustive]`, so it is built with
  `DeviceAnnounce::new` (or `DeviceAnnounce::for_gige`, below) and the optional
  fields assigned, and the next field will not be a breaking change. Only
  `DeviceAnnounce` got the attribute; its sibling payload types did not, and
  that remainder stays in the backlog as `API-13`.

  Two helpers hold the GigE identity rule in one place: `gige_device_id` (the
  `cam-<mac hex>` id the service has always keyed by, now written once instead
  of three times) and `DeviceAnnounce::for_gige`, which builds the whole
  announce from Discovery ACK fields and is what both the service and Studio's
  embedded backend now call.

- **`quick-xml` 0.41 → 0.42, which moves the parser from bytes to `&str`**
  (backlog `CI-14`). 0.42 rewrites the API around `&str`: `QName` and the text
  event types wrap it, `AsRef<[u8]>` is gone, and `BytesText`/`BytesCData`/
  `BytesRef` deref to `str` rather than needing a fallible `.decode()`. Our XML
  layer matched element names against byte-string literals throughout, so this
  touched all ten files of `viva-genapi-xml`: 21 tag constants and 222 literals
  become `&str`, four helper signatures follow (`attribute_value`,
  `attribute_value_required`, `skip_element`, `read_text_events`), and the
  per-element `node_name` snapshot becomes a `String` instead of a `Vec<u8>`.

  It is a net **deletion** — 284 lines added against 317 removed — because the
  decode error paths and the `String::from_utf8_lossy` calls that existed only to
  turn an element name back into something printable are gone with it. quick-xml
  0.42's MSRV is 1.86, below our 1.88, so nothing moves there.

  **The gate for a refactor this wide is that nothing changes**, and that is what
  the vendor corpus is for: 38 documents, 35 407 nodes, 11 382 bitfields, with
  per-document node and bitfield counts and known-gap lists **diffed against 0.41
  and byte-identical**. Worth stating plainly that this is not a bug fix and
  brings no behaviour change; it was done for the simplification.

- **`zip` 2 → 8, and declared once instead of three times.** It was pinned six
  majors back and repeated independently in `viva-genapi-xml` (behind the `fetch`
  feature), `viva-u3v` and `viva-fake-gige` — three copies of a dependency that
  is on the connect path of both transports, because GenICam XML is frequently
  served as a ZIP archive by the camera itself. It is now a single
  `[workspace.dependencies]` entry that all three inherit. No source change was
  needed: the API surface used (`ZipArchive`, `ZipWriter`, `SimpleFileOptions`,
  `CompressionMethod::Deflated`) is unchanged across those six majors, the
  `deflate` feature still exists, and zip 8's MSRV of 1.88 is exactly ours.

### Viva Studio and documentation — not part of any published crate

Viva Studio lives in the `studio/` workspace, which is excluded from the root
workspace and published nowhere: no crate on crates.io and no desktop binary
carries these changes. They are recorded here because the book they correct
*is* published on every push to `main`. Nothing in this section changes the
library.

- **Studio shows write-only features and stops reading command registers**
  (backlog `SVC-08`, `ST-21`; the library half is under Fixed). Embedded mode's
  own copy of `build_feature_state` had the same defects, and there the symptom
  was quieter: `bulk_feature_state` logged a WARN and left the feature out of
  the connect-time snapshot, so a write-only feature had no live state, and an
  Apply to one reported an error after the write had succeeded, because the
  refreshed state could not be read. It now applies the same rule as the
  service. The UI's `FeatureState.value` and `NodeValueEntry.value` admit
  `null`: the tree and the feature header show no value instead of the text
  `null` — which a Command's live badge used to show — the write and execute
  controls stay available, and a live-value preset leaves those features out
  rather than exporting `null` for them.

- **Studio's device list can tell two cameras of the same model apart**
  (backlog `ST-24`, [#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)).
  Each entry in the header dropdown led with the name and model — identical for
  identical cameras — beside a status dot that was always `idle`. It now leads
  with the user-defined name, else the serial, else the address, with the model
  and whichever of serial and address is not already shown on a second line; the
  connected camera is named the same way. `DeviceSidebar.tsx` and
  `DeviceCard.tsx` are deleted, along with the card's CSS: neither was rendered
  anywhere, and `DeviceCard` was the one component that *did* show the serial,
  which made it the obvious wrong place to fix this.

- **Studio's two backends now give a camera the same identity** (backlog
  `ST-25`). Embedded mode used the IP as the device id and fell back to the MAC
  for a missing serial; the Zenoh bridge used `cam-<mac hex>`. Both now build the
  announce with `DeviceAnnounce::for_gige`, so the id is MAC-derived in either
  mode — the IP is the one identifier that changes under DHCP, `FORCEIP`, and a
  persistent-IP write from Studio's own network dialog — and the IP travels as a
  field. Embedded `connect` resolves the IP from the discovery cache. The
  network dialog's MAC was being read from the `serial` field, so it showed the
  serial for any camera that reports one; it reads `mac` now. A repeated
  discovery event now replaces the entry instead of being ignored, so an address
  change is shown.

- **`viva-fake-gige` takes `--mac`, `--model`, `--serial` and `--user-name`**
  (and the matching builder methods), defaulting to the `FAKE_*` constants. Two
  fakes used to be byte-identical in identity, which could reproduce #137 but
  not demonstrate the fix. The MAC is on the list because services key a camera
  by it: two fakes with distinct serials and one MAC are one camera to
  `viva-service`. The crate is `publish = false`.

- **The planning documents now only describe open work.** `docs/backlog.md` had
  accumulated 69 completed rows, each carrying a post-mortem paragraph, and the
  working list had reached 131 KB across 275 rows — about 60% of it a second,
  worse changelog. Those rows are deleted; this file is the record of what
  shipped. `docs/roadmap.md` opened with "This file only looks forward" and then
  spent 110 of its 310 lines narrating two closed releases, so the closed phases
  are gone and the file now names the release in flight and what gates it. The
  remaining phases lost their *"Fixed (TC-xx)"* annotations for the same reason.

  Two things were preserved rather than dropped on the way out. The lessons that
  were about *how to file a row* rather than about any one defect are now in the
  backlog's preamble — do not propose downgrading a log level before the
  warning's cause is known, because that is a proposal to delete the evidence;
  and a row filed from fake-camera output alone is a hypothesis. And the 16 open
  rows that referenced a deleted one were rewritten to carry the fact instead of
  the row ID, so the prune could not quietly turn into data loss.

  Also corrected while in there: the `API` section was still titled "0.4.0
  consolidation" two releases later, `API-03` still asked for a `thiserror` bump
  that shipped in 0.5.0, and the `GA` section asserted a corpus denominator of 37
  where the tree holds 38 — in a paragraph whose own subject is that these
  numbers go stale. It now gives the command instead of a number.

- **Three ADRs that had been reversed in fact are marked reversed in writing.**
  [ADR-0003](docs/adrs/adr0003-gentl-transport.md) said the transport was GenTL
  "exclusively" and was still marked `Accepted`, although GenTL appears nowhere
  in `crates/` — [ADR-0011](docs/adrs/adr0011-pure-rust-genicam-stack.md)
  replaced it. [ADR-0002](docs/adrs/adr0002-camera-service-architecture.md)
  described the camera service as external and in a separate repository; it is
  `crates/viva-service` here. [ADR-0001](docs/adrs/adr0001-desktop-primary.md)
  named a WASM crate that was never carried into this repository. Each now
  carries a "What changed" note, because a superseded decision is part of the
  record and deleting it loses the reversal.

- **`docs/studio/camera-service-api.md` and `studio/CHANGELOG.md` are deleted.**
  The first specified the API of a GenTL-based service in another repository —
  both premises retired. The second is the changelog of "GenICam Studio" before
  the rename and the monorepo import, and every path it names
  (`crates/genicam_xml_model`, `apps/genicam-ws-streamer`, …) is gone; nothing is
  published from it and this file carries the Studio entries now. The seven
  stale `docs/zenoh-api.md` / `docs/camera-service-api.md` links left over from
  that move are fixed, and the gap that let them rot for weeks is filed as
  `CI-17` — nothing checks links outside `book/`.

- **Triage of seven open issues is recorded as backlog rows rather than prose.**
  New: `GA-28` (a plain `<IntReg>` discards its declared byte order — 311
  declarations in 16 of 38 corpus documents decode byte-swapped, found while
  tracing [#140](https://github.com/VitalyVorobyev/viva-genicam/issues/140)), `GA-31`/`DX-11`/`SVC-08` ([#135](https://github.com/VitalyVorobyev/viva-genicam/issues/135)),
  `TC-22`/`TC-23` ([#136](https://github.com/VitalyVorobyev/viva-genicam/issues/136)), `SVC-07`/`ST-24`/`ST-25`/`API-13`
  ([#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137)), `DC-05`/`GA-29` ([#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138), [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)), plus
  `GA-30`, `CI-17` and `REL-08`. `GA-20` moves to **P0** with its open design
  question closed — [#140](https://github.com/VitalyVorobyev/viva-genicam/issues/140) is the second vendor to report it and its
  spec citation settles the representation — and `GA-11` moves to **P0** because
  it is why neither defect was ever caught: the corpus test evaluates against
  zeros, so it stayed green across a union of 448 wrong nodes.

- **Viva Studio now shows which backend mode it started in, and says so when it
  could not honour `ZENOH_CONFIG`** (backlog `DOC-18`/`ST-23`,
  [#132](https://github.com/VitalyVorobyev/viva-genicam/issues/132)). Six
  documents told readers that Studio "loads its own Zenoh config automatically
  in dev mode". It never has: remote mode requires the `ZENOH_CONFIG`
  environment variable to name a loadable config file — there is no default
  path, no dev-mode detection, and no `--zenoh-config` flag on that binary.
  Without it the app starts in embedded mode, whose GigE discovery deliberately
  skips loopback, so the fake camera in the documented walkthrough could never
  appear and the device list stayed empty with no error at all.

  The walkthroughs now export `ZENOH_CONFIG`, absolute — they `cd` into the app
  directory immediately after, which breaks a relative path. More to the point,
  the app no longer hides the answer: the header carries an Embedded/Remote
  chip, and a `ZENOH_CONFIG` that is set but fails to load is now an `error!`
  plus an error toast and a Diagnostics entry, instead of a `warn!` and a silent
  switch to the other mode. It still starts, so it is never unlaunchable.

  Two things surfaced while fixing it. The cookbook's mock-service quick start
  was broken the same way and is now verified end to end rather than assumed.
  And the Tauri crate's 50 unit tests had never run in CI — it is excluded from
  the studio workspace, so `cargo test --workspace` never reached it — which
  `studio-ci.yml` now corrects.

- **The `studio/` workspace now has a `[workspace.dependencies]` table, and its
  crates are on edition 2024** (backlog `ST-01`). It had none, so five crates
  declared their own versions and quietly drifted from the library workspace:
  `thiserror` stayed on 1 after the root moved to 2, `axum` on 0.7,
  `tokio-tungstenite` on 0.26, and `zenoh` was spelled `"1.8"` where the root says
  `"1"`. All five are now on edition 2024 with inherited versions, `axum` 0.8,
  `tokio-tungstenite` 0.30 and `thiserror` 2; the Tauri crate, which is its own
  workspace, takes `dirs` 7.

  Two things had to change in source. axum 0.8's `Message::Text` takes
  `Utf8Bytes` and `Message::Binary` takes `Bytes` — which the frame channel
  already carries, so passing it through **drops a full copy of every frame** that
  the old `to_vec()` was making on each WebSocket send. And edition 2024 enables
  let-chains, so `clippy::collapsible_if` now fires on nested `if let`; the 12
  sites it found are mechanical and were collapsed.

- **The Studio UI's 14 type errors are fixed, and CI now type-checks the
  frontend** (backlog `ST-20`). `bun run build` is Vite, which strips types
  without checking them, so nothing in CI had ever type-checked the UI;
  `studio-ci.yml` now runs `bunx tsc --noEmit`. Seven of the fourteen were a
  stale `uigraph.ts`: the Rust model has always carried `UiCategory::{tooltip,
  comment}` and `UiNode::comment`, so the components reading them were correct
  and the TypeScript declarations were the half that had drifted — the feature
  tooltips were arriving all along. `formatDeviceChip` handled four of
  `ConnectionState`'s five variants and returned `undefined` for
  `reconnecting`; it now renders the attempt count, as `DeviceDropdown` already
  did. The rest were unused declarations, including a fixture-loading
  affordance that had been wired to nothing since the studio was imported.

## [0.5.0] - 2026-08-26

Two behaviour changes need a read before upgrading, both under the headings
below: streaming no longer writes `GevSCPSPacketSize` unless you ask it to, and
big-endian masked registers are now read from the MSB as GenICam specifies —
which changes what `pIsAvailable`/`pIsLocked` evaluate to on most GigE cameras.
Both fix defects reported from real hardware.

### Changed

- **Dependency majors brought current** (backlog `CI-13`): `thiserror` 1 → 2,
  `if-addrs` 0.11 → 0.15, `socket2` 0.5 → 0.6, and `pyo3`/`numpy` 0.28 → 0.29
  for the Python bindings. `if-addrs` was checked rather than assumed — its
  `link-local` feature is what makes APIPA interfaces visible on Windows, and it
  still exists at 0.15. `quick-xml` stays at 0.41 deliberately: 0.42 moves its
  whole API from `&[u8]` to `&str` and is a refactor of every parser, tracked as
  `CI-14`.

- **The project status statements now match the evidence.** `README.md` said the
  library was "barely tested against physical cameras — we have none". The
  tracker says otherwise: users have run discovery, control and streaming
  against real FLIR, Hikrobot and JAI cameras on Linux, Windows and macOS. The
  three statements (root README, crate README, book) also carried three
  different and all-stale test and corpus counts. They now say the same thing,
  from the same numbers, and keep the caveat that actually matters — the API
  moves, and there is no camera in CI.

- **Breaking: streaming no longer overwrites the camera's `GevSCPSPacketSize` by
  default** (backlog `SR-14`, ADR-0021,
  [#118](https://github.com/VitalyVorobyev/viva-genicam/pull/118) — contributed
  by the [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)
  reporter). Since 0.4.0 every `StreamBuilder::build` wrote the host NIC MTU
  into the register, so a camera an operator had tuned to 9000 for a narrow
  switch lost that value on the next Acquisition Start — and Studio rebuilds the
  stream on *every* Start. The default is now **preserve**: the camera's value is
  read and used as-is.

  Two new opt-ins restore the old behaviour explicitly. `StreamBuilder::auto_packet_size()`
  / `viva-camctl stream --auto` / Python `auto_packet_size=True` sets the size
  from the NIC MTU; `packet_size(n)` / `--packet-size N` / `packet_size=N`
  remains an explicit ceiling, and the two are mutually exclusive. `--auto` had
  been removed in 0.4.0 and returns with an honest meaning — it is **not** the
  pre-0.4 flag whose `false` branch fell back to 1500. `viva-service`, Studio's
  Start, `bench` and the examples all opt into auto, so no unattended caller
  silently loses jumbo.

  **All three policies still probe the path** (`SR-13`), preserve included. The
  probe only ever *lowers* a size, so it cannot override one you set; it can only
  decline to stream at a size the path drops, which no register read can find.
  `StreamBuilder::probe(false)` turns it off and makes preserve literal.

### Added

- **`Camera.execute(name)` in the Python bindings, and `viva-camctl execute`**
  ([#121](https://github.com/VitalyVorobyev/viva-genicam/issues/121), backlog
  `API-12`). GenApi `<Command>` features — `UserSetLoad`, `TimestampLatch`,
  `TriggerSoftware` — had no verb in Python, so a user trying to reset a camera
  through `UserSetSelector` + `UserSetLoad` concluded, reasonably, that it was
  not possible.

  It *was* possible: `Camera.set(name, "1")` dispatches Command nodes and
  discards the value, and always has. Nothing said so, and a setter that
  requires a meaningless value is not an API anyone should have to guess. The
  Rust facade already had `Camera::execute_command`, and the service and Studio
  already spoke `execute` on the wire; only Python and the CLI were missing it.
  Both now use the same verb.

  `viva-camctl execute --name <Node>` does not read back: `Camera::get` on a
  Command is a type error, and GenICam's `<pIsDone>` polling is not implemented
  anywhere in this library, so the only honest report is that the write was
  acknowledged. That limitation is now documented rather than implied.

- **The GVSP packet size is now probed against the network path, not just the
  device** (backlog `SR-13`,
  [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)).
  `GevSCPSPacketSize` reports what the *camera* stored, which is not what the
  *link* will carry: on a Vieworks FS3200T the camera declares `Max=16366`,
  accepts and holds 16114, and streams nothing, because the path tops out at a
  9216-byte frame. `StreamBuilder` now asks the camera for a GVSP test packet
  (bit 31, with do-not-fragment on bit 30) and bisects to the largest size that
  actually arrives — 9198 for those numbers.

  **A device that never answers keeps the size it was given.** The probe first
  requests a test packet at 1500; if that produces nothing, the device does not
  answer probes and nothing is changed. Reading silence as "too big" would have
  walked every camera that has never implemented test packets down to 1500.
  The probe also never *raises* a size, so an explicit `--packet-size` stays a
  ceiling, and `StreamBuilder::probe(false)` restores the previous behaviour.

- **`viva-fake-gige`: `max_on_wire`** makes the fake drop oversized GVSP
  datagrams in flight while still accepting the size into its register — the
  shape of the reporter's link, and something the existing `max_packet_size`
  register clamp cannot express. Without it `SR-13` would have been untestable.

### Fixed

- **Breaking (behaviour): big-endian masked registers were read off the wrong
  end** (backlog `GA-22`,
  [#120](https://github.com/VitalyVorobyev/viva-genicam/issues/120), reported on
  a FLIR BFS-PGE-31S4C-C). Writing `ExposureTime` was refused locally as
  `node unavailable` on a camera that SpinView, Spinnaker and `arv-tool` all
  write. Two separate defects in the same path, neither of which produced an
  error, a warning or a skipped node:

  1. **Bit numbering.** GenICam counts `<LSB>`, `<MSB>` and `<Bit>` from the
     **most** significant bit when a register declares
     `<Endianess>BigEndian</Endianess>`. The XML layer converted the index as
     though it were LSB-relative and `bitops` converted it again, so the two
     cancelled out. **9 473** big-endian single-bit fields across the vendor
     corpus were read from the wrong end.
  2. **Element casing.** `parsers::numeric` matched only `<Lsb>`/`<Msb>`, while
     all 1 419 declarations in corpus register nodes use the schema spelling
     `<LSB>`/`<MSB>`. Those registers therefore carried **no bit range at all**
     and returned their entire register value — **1 374** fields across the
     corpus.

  On the reporter's camera, `ExposureTime` gates on three `<MaskedIntReg>`
  predicates sharing one big-endian word at `0x000C1000` (bits 0, 1 and 3 from
  the MSB). All three read as zero, so the feature looked unimplemented and every
  setter refused before anything reached the wire — which is why setting
  `ExposureAuto=Off` first made no difference.

  **This did not need hardware to settle.** `AVT_Manta_G125B.xml` declares
  bootstrap `GevSCPSPacketSize` at `0xD04` as `<LSB>31</LSB><MSB>16</MSB>`
  big-endian; GigE Vision fixes the packet size as the low 16 bits; and the
  reporter's own diagnostic bundle shows that register holding `0x40000578`,
  i.e. 1400 bytes. Counted from the MSB that yields 1400, and our reading yielded
  16384. The corpus agrees independently — 1 307 big-endian declarations with
  `LSB > MSB` and none the other way, 41 little-endian with `LSB < MSB` and none
  the other way — and so does aravis.

  `<Mask>` deliberately keeps the endianness conversion: a mask is a literal
  register value and so is LSB-relative by construction. It has zero corpus
  occurrences, which is exactly why it now has its own test.

  **A consequence worth stating plainly:** the `pIsLocked` guard added for
  [#45](https://github.com/VitalyVorobyev/viva-genicam/issues/45) can never have
  fired on that reporter's camera. It was reading bit 3 from the wrong end and
  always saw zero.

- **GenApi XML beginning with a UTF-8 byte-order mark loaded as an empty
  nodemap** ([#122](https://github.com/VitalyVorobyev/viva-genicam/issues/122),
  reported on a The Imaging Source DMK 33GP2000e). A BOM is valid UTF-8, so it
  survived `String::from_utf8` and reached the parser. quick-xml removes it from
  its own view of the input but does **not** advance `Reader::buffer_position`,
  and `viva_genapi_xml::parse` slices each node element out of the caller's
  `&str` by exactly those offsets — so every slice started three bytes early,
  lost the closing `>` of its end tag, and was recorded as unparsable. The
  failure was quiet in the worst way: `parse` returned `Ok`, the offset-free
  top-level scan listed all 291 features correctly, and then every single node
  was skipped, which is what the reporter saw. The BOM is now stripped in both
  `parse` and `parse_into_minimal_nodes`.

  The reporter's XML is in the vendor corpus as `TIS_DMK_33GP2000e.xml`; it is
  the only document there that opens with a BOM, which is why nothing caught
  this earlier. Its one remaining skipped node — a `<Register>` with `<pLength>`
  — is the separate, known `GA-09` phase-two gap and is unaffected by this fix.

  The element slice is now taken with `str::get` rather than by indexing. No
  corpus document reaches a non-character-boundary index, so this is defensive
  only: it makes any future offset disagreement cost one skipped feature instead
  of panicking part-way through a camera connect.

- **The GVSP path probe did not write its own answer back, leaving the camera
  configured at a size it had merely tested** (backlog `SR-15`,
  [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)). Asking for
  a test packet *is* a write to `GevSCPSPacketSize` — bit 31 rides in the same
  register as the size — so when the probe finished, the device held the last
  size it was asked *about*, not the one it chose. Two ways that goes wrong, and
  neither is visible from the host: a bisection whose final probe fails ends one
  byte above its own answer, and a device that never answers is left at the
  1500-byte control size while the host strides at the size it requested.

  On the numbers in #112 the probe negotiated 9198 and left the camera at
  **9199** — the first size that reporter measured as failing. The second case
  is worse because it is the common one: the probe exists precisely so that a
  camera which has never implemented test packets is left alone, and instead
  every such camera was reconfigured to 1500 behind the caller's back. That is
  the `SR-02` failure mode, caused by the fix for `SR-13`.

  The probe now ends by configuring the value it returns, which also clears the
  do-not-fragment bit it set — except where the device already holds that value,
  which is reachable only after a DF-set test packet of that size crossed the
  path. Both existing `SR-13` tests asserted
  `params().packet_size` only and passed throughout.

- **The fake camera reported a GVSP trailer `size_y` of zero** (backlog
  `TC-04`). `size_y` is the number of lines a block actually delivered — the
  only field that tells a receiver the true height of a variable-height block,
  which is how a linescan or profile scanner reports how much it sent.
  `viva-fake-gige` hardcoded it to `0`, and since `TC-17` had already taught
  the parser to read that field from the right offset, producer and consumer
  agreed on a wrong value with nothing looking. Found by the new
  spec-derived GVSP wire test, which is the fourth instance of the pattern
  ADR-0019 exists for.

### Testing

- **The fake camera can now disagree with us about bit numbering** (backlog
  `GA-22`, ADR-0019). Every predicate in `viva-fake-gige` was an
  `<IntSwissKnife>` or a `<StructEntry>`, and both took the code path that was
  already correct — so the whole suite passed while `<MaskedIntReg>` read real
  cameras' registers off the wrong end. `ExposureTime` now gates on big-endian
  `<MaskedIntReg>` + `<Bit>` predicates over a FLIR-shaped feature-status word,
  and reverting the fix fails two `predicates.rs` tests instead of none.

- **The XML corpus test asserts that a declared bit range survives parsing.**
  Dropping one is silent by construction — the node still parses and simply
  reads its whole register — so no skip list could have caught the casing defect
  above. The check is per document rather than a corpus-wide total, because the
  fetch script warns and continues when a third-party URL is unreachable and a
  size-keyed assertion would fail for reasons unrelated to the parser. Reverting
  the fix fails 24 of the 38 documents.

- **Two test fixtures encoded the defect rather than catching it.**
  `parse_integer_bitfield_big_endian` and the `BeBits` nodemap fixture both used
  a big-endian `<Lsb>`/`<Msb>` pair oriented against its own byte order — a shape
  that appears **zero** times in the vendor corpus. Both now use real vendor
  shapes, and the numbers they assert come from the GigE Vision register layout
  rather than from our own output.

- **The fake camera has `UserSetSelector` and `UserSetLoad`** — its first
  `<Command>` that reaches its register through `<pValue>` rather than a bare
  `<Address>`. All 432 `<Command>` nodes in the vendor XML corpus use `<pValue>`
  and all three of the fake's used the direct-address path, so the integration
  suite exercised only the path no real camera takes (backlog `GA-10`).
  `UserSetLoad` restores the analog-control defaults, so a test can move a
  feature, execute the command, and read the change back off the device — a
  command that only acknowledges its write can be "verified" by a test that
  proves nothing (ADR-0019).


- **`test_fake_gvsp_packets_match_spec_layout`** (backlog `TC-04`,
  [#63](https://github.com/VitalyVorobyev/viva-genicam/issues/63)) asserts the
  fake's GVSP leader, data packets and trailer against the specification's
  field tables, reading them from a `UdpSocket` the test binds itself.
  `StreamBuilder`, `Stream`, `FrameStream` and `gvsp::parse_packet` are all
  deliberately absent: a round trip through our own receive path proves only
  that it agrees with our own fake.

- **`viva-fake-gige` enforces `max_on_wire` on the stream, not only on test
  packets** (backlog `SR-15`). The ceiling modelled a narrow hop for the probe
  and then let every frame through regardless, so a camera left configured above
  it still delivered perfectly — the fake was physically incapable of
  contradicting the probe's own answer, which is the ADR-0019 failure mode one
  level up from the parser. `test_probe_finds_a_path_ceiling_the_device_does_not_report`
  now requires a frame to arrive, and two new tests read `GevSCPSPacketSize`
  back over raw GVCP rather than trusting `params()`.

- **Five spec-derived GenCP acknowledgement-header tests** (backlog `TC-04`)
  pin the header as a literal byte array indexed by offset, rather than
  building the input with the same calls the decoder reads back. They also fix
  the *interpretation* of `length` — the payload alone, not the datagram —
  because off by exactly `HEADER_SIZE` a fake and a client round-trip
  perfectly while every real device disagrees.

## [0.4.1] - 2026-08-03

### Added

- **`--iface` accepts a host IPv4 address *or* an OS interface name, in every
  tool** (backlog `DX-10`, [#109](https://github.com/VitalyVorobyev/viva-genicam/issues/109)).
  It previously meant a different thing in each: `viva-camctl` took the host
  NIC's IPv4 address and rejected a name, while `viva-service` and the Python
  `iface=` argument took a name and rejected an address. A value obtained with
  one tool was not a legal argument to the next — and on Windows the name is a
  GUID like `{6394C55F-F630-4BC7-92D2-7AC320C73D1C}`, the spelling a user is
  least able to supply.

  The new `viva_gige::nic::IfaceSelector` parses an IPv4 literal first and
  takes anything else as an interface name; the two cannot collide, since no
  interface name parses as an address. Every `--iface` and `iface=` in the
  workspace now goes through it, including the six Rust examples, which had
  split the same way four-to-two. **Nothing that worked before stops working**
  — this widens what is accepted, it does not move it.

  A selector that resolves to nothing now lists every interface the library can
  see, with its addresses. That is the part that answers the report: the old
  `no interface with IPv4 169.254.105.106` could not tell anyone what their
  adapter was actually called.

- **A silent stream now says what to check** (backlog `DX-09`). A stream that
  received nothing printed `frames=0 drops=0 resends=0`, and that line was
  identical for a firewall block, a packet size the path cannot carry, a
  control privilege held by another application, and a camera waiting for a
  trigger. After a few seconds of producing nothing the receiver warns once,
  and it distinguishes the two cases it can already tell apart: *no GVSP packet
  has arrived* (path or privilege) versus *packets are arriving but no frame
  has completed* (a packet-size disagreement). Both receive paths carry it, the
  Windows reader thread included.

- **`viva-fake-gige --max-packet-size`** makes the fake clamp
  `GevSCPSPacketSize` the way a real camera does — acknowledging the write and
  silently reducing it. Without this the fake accepted any size and so could
  not express the camera behind
  [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), which is
  the ADR-0019 failure mode of a fake that only ever agrees with its client.

### Fixed

- **A camera that clamps the GVSP packet size produced a stream that never
  completed a frame** (backlog `SR-02`). Found by reading code, not on
  hardware — it is *not* the cause of
  [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112), whose
  camera accepts the size it is given.

  `StreamBuilder::build` wrote `GevSCPSPacketSize` and never read it back.
  `StreamParams.packet_size` kept the **requested** value and `gvsp_payload_size`
  derives every reassembly offset from it, so a camera that clamped left the
  host striding at the wrong pitch. The write still succeeded — nothing on the
  wire distinguishes "accepted" from "accepted and reduced".

  `build` now reads the register back over GVCP READREG (never through GenApi,
  whose cache the raw write bypasses) and puts the **effective** size in
  `StreamParams`, warning when it differs from the request. A device that will
  not answer the read-back keeps the requested value and says so, rather than
  failing a stream that works today. The read also comes *first*: Viva Studio
  rebuilds the stream on every Acquisition Start, so an unconditional write
  discarded a working configuration once per Start.

- **An explicitly configured packet size bypassed the IPv4 clamp** (backlog
  `TC-08`'s leftover). `GevSCPSPacketSize` holds the size in 16 bits, so
  `--packet-size 70000` silently configured 4 464. It is now refused with an
  error naming both the value and the bound.

- **`viva-service` could not stream without `--iface`** (backlog `SVC-06`).
  It resolved the receive interface with `Iface::from_ipv4(camera_ip)` — the
  camera's address handed to the lookup that searches the *host's* own
  addresses — so on any real network it failed with `no interface with IPv4
  <camera-ip>` and streaming never started. It now probes which local
  interface routes to the camera, matching `viva-camctl` and the Studio
  embedded backend.

  This is the same confusion as [#70](https://github.com/VitalyVorobyev/viva-genicam/issues/70)
  for the third time — the studio backend was fixed in 0.3.1 and `viva-camctl`
  in 0.4.0, and `Iface::from_remote_ipv4` was added *for this case* — so
  `DeviceHandle` now carries a resolved interface rather than a name, leaving
  no second place to re-resolve it differently. Note that the fake camera
  cannot reproduce this and never could: it lives on `127.0.0.1`, which *is* a
  host address, so the broken lookup succeeds there by coincidence. The
  regression test uses `127.0.0.2` — routable, but on no interface.

## [0.4.0] - 2026-07-31

A minor bump rather than the 0.3.2 the roadmap planned. This release removes
`StreamBuilder::auto_packet_size` and the Python `open_stream(auto_packet_size=)`
argument, and adds variants to two public enums. Cargo reads `^0.3` as any
0.3.x, so shipping those as a patch would break dependents on the next
`cargo update`. As a consequence the API consolidation the roadmap called
"Phase 5 — 0.4.0" renumbers to 0.5.0.

### Added

- **`<Register>` node support, for declarations with a plain `<Length>`**
  (backlog `GA-09`, first cut). `<Register>` is the base register type — an
  address, a byte count and no interpretation of the bytes — and it was dropped
  entirely, taking `FileAccessBuffer` with it on two vendors' hardware
  (a JAI on #70, a Micro-Epsilon scanCONTROL) and prompting the
  `NodeMap::register_address` request in #92. `NodeMap::get_register` and
  `set_register` read and write the raw bytes; `RegisterNode` is re-exported.

  **`<pLength>` — a length resolved from another node at runtime — is
  deliberately still unsupported**, and says so: such a node is recorded in
  `NodeMap::skipped()` with an error naming `<pLength>`, rather than parsed and
  read at the wrong size. That covers 42 of the vendor corpus's 63 `<Register>`
  declarations, clearing every skipped node in seven documents; the remaining 21
  are concentrated in three vendors. The row previously claimed 56 declarations
  with only 5 usable — see the correction in #101.

  `set_register` refuses a slice that is not exactly the declared length rather
  than zero-padding as `set_string` does: padding a 100 000-byte file-transfer
  buffer because the caller supplied twelve bytes is data loss.

  A register bound to a non-device `<pPort>` — the scanCONTROL's three
  `Chunk*Results` blocks — is parsed and listed but **cannot be read**. Its
  address is relative to a port we do not route, and all three sit at `0x0`, so
  a device-port read would return GVCP bootstrap registers dressed as
  measurement data. Port routing is `GA-12`.

- **`Mono12` and `Mono14`** — the two pixel formats the Micro-Epsilon
  scanCONTROL 850050 advertises that #93 did not cover. Its `PixelFormat`
  enumeration offers ten formats; eight were named.

### Changed

- **`Node`, `NodeDecl`, `ChunkKind`, `ChunkValue` and `ChunkError` are now
  `#[non_exhaustive]`**, so adding a variant to any of them stops being a
  breaking change for downstream crates. Marking them is itself only possible in
  a breaking release, and the `<Register>` work above is the argument for doing
  it now: one new node type touched seven `match` sites across three workspaces.

  **Deliberately not marked.** `AccessMode`, `Sign`, `ByteOrder`,
  `FloatEncoding`, `SkOutput`, `StreamDest` and `GvspPacket` are closed sets
  fixed by the GenICam and GigE Vision specifications — RO/RW/WO, unicast or
  multicast, leader/payload/trailer. Marking those would cost every consumer a
  wildcard arm forever in exchange for flexibility nobody will use.

  The cost, stated plainly: `#[non_exhaustive]` binds only *other* crates, so
  this workspace's own cross-crate matches now need wildcards, and the compiler
  will no longer name them when a variant is added — which is exactly the help
  that found every call site for `<Register>`. Each such arm is therefore
  written to fail loudly rather than default silently. `build_node` returns
  `GenApiError::Unsupported` naming the declaration kind, which surfaces the
  node in `NodeMap::skipped()` and trips the vendor-corpus test's
  unexpected-skip check; `Camera::get`/`set` and both service backends name the
  node kind in their error rather than returning a bare type error or a null.

### Fixed

- **`viva-camctl stream`, `bench` and `events` no longer refuse to run without
  `--iface`.** They returned `streaming requires --iface or global --iface`
  before touching the network, while `list`, `xml`, `report`, `get`, `set` and
  `set-ip` all fall back to broadcast discovery. That is not merely
  inconsistent: `viva-camctl stream --ip <IP>` is the command this project's own
  documentation hands to anyone reporting a camera we cannot open, and the first
  person given it had to diagnose and work around the failure themselves (#70).
  Worse, `Iface::from_remote_ipv4` — the route probe added by #72 *for exactly
  this case*, and produced by that very issue — had no caller in `viva-camctl`
  at all. When `--iface` is absent the local interface is now found by asking
  the OS which one routes to the camera, which also makes `--index` work without
  `--iface` (backlog `DX-08`).
- **A probed MTU is no longer discarded in favour of a hardcoded 1500.**
  `StreamBuilder` had an `auto_packet_size` flag whose `false` branch fell back
  to `best_packet_size(1500)` — throwing away the MTU it had just measured — and
  `viva-camctl`, the studio's embedded backend and every streaming example
  defaulted that flag to **false**. On the 16114-byte link in #70 that turned a
  3.1 MB frame into roughly 2 100 packets instead of 200. The flag is gone; the
  packet size follows the probed MTU unless the caller sets an explicit one, and
  the same rule now applies to `viva-camctl`, `viva-service`, the studio and the
  examples alike (backlog `SR-10`).

  **Breaking:** `StreamBuilder::auto_packet_size` is removed, as is the
  `auto_packet_size` argument of the Python `Camera.open_stream`. Use
  `packet_size(u32)` / `packet_size=` to override, or omit it to follow the
  link. `viva-camctl stream --auto` becomes `--packet-size <BYTES>`, and the
  three streaming examples make the same swap.

  Note the effect is invisible on macOS until `TC-11` lands: `nic::mtu` probes
  only on Linux and Windows and returns 1500 elsewhere, so a macOS host gets the
  old number for a new reason.
- **Chunk data can be enabled against the fake camera, so the chunk path has an
  end-to-end test for the first time.** `viva-fake-gige` declared
  `ChunkModeActive` and `ChunkEnable` as `<Integer>`; SFNC defines both as
  IBoolean, and of the 23 vendor-corpus documents that declare
  `ChunkModeActive`, 23 use `<Boolean>` over a backing `<IntReg>` and none uses
  `<Integer>`. `Camera::configure_chunks` calls `set_bool`, which is right, so
  the call failed with a type mismatch and the only camera the project can test
  against could not turn chunks on. The fake is fixed, not the library. The
  consequence was larger than one command: **no test anywhere exercised chunk
  acquisition**, which is why the trailer-offset defect above was found in a
  user's log rather than in CI (backlog `TC-19`).
- **A pixel format we cannot name is no longer treated as one byte per pixel.**
  Bits 23-16 of every PFNC code are the pixel's bit depth, so
  `PixelFormat::bytes_per_pixel` now derives a size for `Unknown(code)` instead
  of returning `None`. That `None` mattered because essentially every caller
  writes `.unwrap_or(1)`: the Windows receive path truncated frames to
  `width * height` bytes and the U3V builder under-allocated its payload buffer
  by the same factor. It still returns `None` when the depth is not a whole
  number of bytes — `Mono12Packed`, `Mono10Packed`, `YUV411Packed` and the two
  packed Bayer formats all declare 12 bits, and eleven of the 37 vendor-corpus
  documents offer at least one of them. Rounding those up would overstate a
  frame by a third, which a length check reads as a *short* payload; a
  confidently wrong size is worse than an absent one (backlog `SR-11`).
- **The Zenoh bridge no longer truncates a frame whose format it cannot name.**
  It sized every frame through `pfnc_to_zenoh`, which collapses anything the
  wire enum does not carry to `Unknown` — and `Unknown` reports 1.0 bytes per
  pixel. A `Coord3D_ABC32f` profile was therefore cut to **one twelfth of
  itself** and published as valid, with the "trimming trailing bytes" warning
  emitted once and every later frame corrupted in silence. Frames are now sized
  from the camera's own PFNC code, and a format with no whole-byte size is
  published unmodified with a warning naming it, because an expected length we
  cannot compute must not become a length we enforce. Not specific to 3D:
  `Mono10`, `Mono12` and `Confidence8` all took the same path (backlog `DC-01`).
  The arithmetic also moves from `f32` to integers, which no longer loses
  precision above 2^24 bytes.

- **The GVSP data trailer is read at the offset the specification gives it.**
  The trailer payload is eight bytes — `reserved(2) | payload_type(2) |
  size_y(4)` — and the chunk region starts after them. `parse_trailer` read two
  and handed the remaining six to the chunk parser, producing a
  `chunk header truncated remaining=6` warning on every frame: those six bytes
  *are* `payload_type` and `size_y`. Not cosmetic — with chunk mode on the
  six-byte prefix desynchronises every chunk that follows, so chunks could not
  decode on any conforming camera. The same two bytes were reported as the
  trailer's `status`, which both frame-reassembly paths test to reject a bad
  frame; they are reserved and always zero, so that check could never fire. The
  real GVSP status is at offset 0 of the packet header, where `parse_packet` was
  instead shifting it right by four, calling it a "payload type" and passing it
  to a parser that discarded it — so the status was examined nowhere in the
  receive path. `GvspPacket::Trailer` now carries `payload_type` and `size_y`.
  Found in the log attached to #70; `viva-fake-gige` was already emitting a
  correct trailer, making this the first case where the fake was right and only
  the client was wrong (#96).
- **GigE discovery reaches link-local cameras on Linux.** Discovery took the
  broadcast address from `if-addrs`' optional `broadcast` field, and on Linux an
  address configured without an explicit `brd` — a manually assigned
  `169.254.0.0/16`, typically — can report the interface address itself there,
  turning the broadcast into a unicast to self. It is now derived from the
  netmask, which is authoritative. Complements #57, which fixed enumeration of
  link-local addresses; even once enumerated, discovery was not leaving the
  host. Confirmed on hardware by @Katze719, whose Micro-Epsilon scanCONTROL
  8500-50 at `169.254.7.189` is now discovered automatically (#95).

## [0.3.1] - 2026-07-31

### Added

- **Six pixel formats used by Micro-Epsilon scanCONTROL 8500/8200 laser
  profile scanners** — `Mono10`, `Confidence8`, `Coord3D_C32f`,
  `Coord3D_AC16`, `Coord3D_AC32f` and `Coord3D_ABC32f`, with codes, names and
  `bytes_per_pixel` (#93, thanks @Katze719). The `Coord3D_*` group is the first
  3D point-cloud format family in `viva-pfnc`, and a profile scanner is a
  device class the project had had no report from.
- **`NodeMap::register_address(name, io)`** — resolves the device register
  address and length backing a feature, so a caller doing raw register I/O
  through `RegisterIo` does not have to reimplement GenICam addressing (#92,
  thanks @Katze719). Address terms, `<pIndex>` scaling and selector blocks
  resolve exactly as they do for `get_integer`. The motivating case is a
  file-transfer buffer, whose address the XML supplies through `<pAddress>` and
  which no typed accessor covers — the same gap that leaves `<Register>` nodes
  unsupported (backlog `GA-09`). The addressing types themselves stay private,
  so that model can still change without a breaking release.
- **`pip install viva-genicam` now installs the `viva-camctl` CLI.** We tell
  reporters to run `viva-camctl report` or `viva-camctl xml` when we cannot open
  their camera — right after telling them to pip-install — and the wheel shipped
  no such command (#45). It is linked into the extension module rather than
  staged as a per-target binary, so it is present in every wheel *and* in an
  sdist build, with no Rust toolchain needed at the user's end.

  `viva-camctl` gained a library entry point (`viva_camctl::run(argv) -> u8`) so
  the binary and the console script run the same code; the binary's `main` is now
  three lines.
- `viva-genicam` example `fetch_xml` — fetch a camera's GenApi XML through
  `fetch_and_load_xml`, print its schema version, top-level features and the
  nodes the parser had to skip. It is the layer below `connect_gige`, so it
  works on a camera that cannot be opened.

### Fixed

- **GigE acquisition works on a Windows APIPA link.** Streaming resolved the
  receiving interface by passing the *camera's* IP to `Iface::from_ipv4`, which
  expects a host interface address — so on any real network it failed with
  `no interface with IPv4 <camera>`, and it only ever appeared to work against
  the loopback fake, where the two addresses coincide. `Iface::from_remote_ipv4`
  now asks the OS which local interface serves the camera. The GVSP socket also
  binds to that interface rather than `0.0.0.0`, Windows reads its real MTU via
  `GetIfEntry2`, and a `#[cfg(windows)]` blocking receive thread delivers frames
  the async path did not. Reported, fixed and confirmed on hardware by
  @InsuJeong496 (#70, #72) — a JAI on a 169.254.0.0/16 link now streams
  2048×1536 at 75 Mbit/s with no drops.

  This is the fix the release waited on. Every gate CI can run was green on
  merge day; none of them says anything about a `#[cfg(windows)]` path against a
  camera, so 0.3.1 was held until a user with the hardware said it worked.
- **A slow GVCP command is no longer executed twice** (#91, thanks @Katze719).
  Each retry allocated a *fresh* request ID, so a device answering after the
  first receive deadline replied with the ID we had stopped listening for; the
  late acknowledgement was discarded as unmatched and the command was sent
  again. For a non-idempotent operation — a file write, a flash commit — that
  means it ran a second time. Retries now keep the original request ID, so a
  delayed answer still matches its transaction, and stale or unrelated
  acknowledgements are discarded while waiting rather than ending the wait.
  Covered by a UDP regression test that delivers a stale acknowledgement ahead
  of the real reply and asserts the command is not resent.
- **An idle GigE session no longer loses control of the camera.** A GigE Vision
  device revokes control privilege if it receives no GVCP command within
  `GevHeartbeatTimeout` (3 000 ms is typical), and GVSP image traffic does not
  count towards it — so a camera could be streaming at full rate, or simply
  sitting at a Python prompt, and the next `set()` would fail with
  `ACCESS_DENIED`. The library refreshed that timer nowhere: `connect_gige`
  claimed privilege and nothing kept it. `GigeRegisterIo` now owns the
  keepalive. It reads the device's own `GevHeartbeatTimeout` and pings at a
  quarter of it, so a device asking for a shorter window gets one, and it stops
  when the transport is dropped — holding a `Camera` is all a caller has to do.

  This also removes the three app-layer copies that had grown around the gap
  (`viva-service`, `viva-camctl stream`, and the studio's embedded backend, the
  last two added in #72). Each contended for the application's *camera* lock
  rather than the device lock, which is why `viva-service` needed
  `pause_heartbeat`/`resume_heartbeat` around reconnection; the transport-owned
  keepalive needs no such coordination and that API is gone.
- The `get_set_feature` example never got or set a feature. Both the README and
  the book named it as the get/set template, and it fetched XML and ended on
  `println!("Stub: would map register ... to a GenApi feature")`. It now uses
  `connect_gige` and `Camera::{get,set,enum_entries}` and takes
  `--name`/`--value`.
- `demo_fake_camera` wrapped every feature access in `spawn_blocking`, on the
  grounds that "block_on can't nest in async". `GigeRegisterIo::{read,write}`
  have guarded that with `block_in_place` since before the comment was written,
  so the zero-hardware demo was teaching a wrapper nobody needs.

### Changed

- **CI no longer lets one failing gate hide the others.** `ci.yml` and
  `studio-ci.yml` both ran fmt → clippy → test as sequential steps in a single
  job, so the first failure masked everything after it. On #72 that hid a real
  streaming regression for a day — `cargo test` never ran once across eight
  contributor commits. Formatting and `cargo deny` moved to their own job, and
  the remaining gates run even when an earlier one fails.
- **`viva-pygenicam` is linted.** It sits outside the root workspace, so nothing
  in CI had ever formatted or clippy-checked it; it had drifted out of rustfmt
  in 35 places, carried two clippy findings, and its separate `Cargo.lock` went
  stale for a whole PR unnoticed. `python.yml` now runs `cargo fmt --check` and
  a `--locked` clippy over it.
- `GigeDevice` gained `heartbeat_timeout_ms()` and `ping_control_channel()`, and
  `gvcp::consts` gained `HEARTBEAT_TIMEOUT` (`0x0938`) and
  `CCP_CONTROLLER_BITS`. `ping_control_channel` returns `Ok(false)` rather than
  an error when the device reports privilege cleared: the register read
  succeeded, and losing privilege to another controller is an answer, not a
  transport failure.
- `viva-fake-gige` can enforce the heartbeat rule (`--enforce-heartbeat`,
  `FakeCameraBuilder::enforce_heartbeat`) and report a custom
  `GevHeartbeatTimeout`. It is off by default so that unrelated tests are not
  sensitive to a multi-second stall on a loaded CI runner. Without it a fake
  camera cannot tell a working keepalive from a missing one, which is how this
  defect survived three reimplementations of the loop above it.
- **The book documented an API that never existed, and now cannot again.**
  Chapters showed `viva_genicam::Client::connect`, `cam.get_f64`,
  `viva_gige::control::ControlClient`, `net::InterfaceSelector`,
  `genicam::Context::new` and an example called `stream_basic` — so the first
  snippet a reader copied failed to compile. The `viva-gencp` chapter was
  invented end to end: not one of `Request`, `Reply`, `Status`, `bitops`,
  `chunk::ChunkPlan` or `helpers::read_u32` exists. Every Rust snippet in the
  book is now an mdBook `{{#include}}` of an anchored region in
  `crates/viva-genicam/examples/`, which `cargo clippy --workspace
  --all-targets` compiles, so a snippet cannot drift from the API again without
  failing a gate. Enforced by `crates/viva-genicam/tests/book_includes.rs`
  rather than by mdBook, which exits 0 on a missing include file and renders a
  missing anchor as an empty code block with no diagnostic at all.
- **Prose claims corrected against the code.** The Python docs advertised
  chunks, events and time sync, none of which the bindings expose, and promised
  a vendor-alias fallback in `set_exposure_time_us` that does not exist. The
  streaming tutorial taught readers to watch the `resends` statistic, which
  nothing in a live stream increments — `resends=0` means "not implemented",
  not "none were needed". `GevFirstURL` is at `0x0200`, not `0x0000`.
  `viva-camctl --json` is a top-level flag and every example placed it after the
  subcommand, where clap rejects it. Two of `book/src/api.md`'s five rustdoc
  links were 404s on the published site.

## [0.3.0] - 2026-07-30

The GenApi layer now implements the GenICam formula language and register
address model rather than plausible approximations of them. **Every user on
0.2.x should upgrade**: 27 of the 30 real camera descriptions in our vendor
corpus could not be opened at all on 0.2.8, and those that could were reading
the wrong registers.

This release grew out of a Hikrobot MV-CS050-10GC report (#35) whose author
attached a dump of their parsed model. Cross-checking it against the vendor
corpus showed that none of the defects it exposed were vendor-specific. See
[ADR-0018](docs/adrs/adr0018-genapi-conformance-over-convenience.md) for the
full audit and the policy that came out of it.

### Added

- **`viva-camctl report` — one command that produces a bug report.** The
  maintainer has no cameras, so every camera-specific defect fixed so far was
  diagnosed from an artifact the reporter assembled by hand. This collects all
  of them in one pass: the network interfaces *as the library sees them* (an
  interface we cannot enumerate is invisible to discovery no matter what the OS
  reports — that was #57), the discovery reply, 24 bootstrap registers with
  decoded values, the GenApi XML, and every feature the camera has that we
  could not build. Each section records either its findings or why it has none,
  so a camera that cannot be opened still produces a report — that camera being
  the one worth reporting. Output is plain text in one file, because that is
  what an issue tracker accepts as an attachment.
- **`viva-camctl xml` — dump a camera's GenApi XML.** The fetch existed but was
  reachable only through the nodemap path, which is precisely the step that
  fails on the cameras whose XML we most need. The reporter of #45 was told
  camctl could do this; it could not, so their BFS-PGE-31S4C-C description is
  still missing and four other models stood in for it. This command parses
  nothing.
- `Iface::list` and `Iface::all_ipv4` expose the library's own view of the
  host's interfaces, including every IPv4 on a multi-homed NIC.
- **The fake camera implements actions and events.** It acknowledges an
  `ACTION_CMD` addressed to its device and group keys and ignores one that is
  not, and emits `GEV_EVENT_START_OF_TRANSFER` per frame once
  `EventNotification` is `On` for it — both backed by real registers and real
  SFNC features. Per [ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md),
  a protocol feature the fake does not implement is not considered implemented;
  neither of these had ever been exercised, which is how both opcode errors
  survived. Golden-byte fixtures for `ACTION_CMD` and `EVENT_CMD` are asserted
  against literal spec-derived arrays, independently of our own encoder.

### Fixed

- **A camera that asks for more time is granted it** (#60). GVCP lets a device
  answer a slow command with `PENDING_ACK` (0x0089) instead of the real
  acknowledgement, requesting an extension. We did not know the opcode, so the
  decode failed and the command was reported as a protocol error — cameras use
  this for exactly the operations you least want to see fail, flash writes and
  mode changes. The controller now extends its own deadline and keeps waiting
  on the same request id rather than resending, since a resend could execute
  the command twice. Bounded by `MAX_PENDING_ACKS` and `MAX_PENDING_ACK_WAIT`.
  Handled in the GVCP layer rather than in `viva-gencp`: the U3V side of GenCP
  signals the same condition with status `0x8006`, so the two transports do not
  share a representation.
- **GigE discovery works on Windows APIPA networks** (#57). A camera and host
  both on IPv4 link-local addresses (`169.254.0.0/16`) were undiscoverable: the
  `if-addrs` `link-local` feature was off, and that crate drops `169.254.x.x`
  on Windows without it. Reported against a JAI FS-3200T-10GE-NNC.
- **The Discovery ACK MAC address is read from the right offset** (#57). It
  begins at payload offset 10, not 12; the last two bytes of the reported
  address were actually the first two of `SupportedIPConfiguration`. A camera
  at `00:0C:DF:06:5B:2F` was reported as `DF:06:5B:2F:C0:00`. The fake camera
  emitted the same shifted layout, so producer and consumer agreed with each
  other while both disagreed with the standard — the third occurrence of that
  pattern, and the reason for
  [ADR-0019](docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md).
- **One failing interface no longer aborts the whole discovery** (#57). A bind
  failure, an `SO_BROADCAST` rejection, a send error or a receive error on one
  NIC now logs and continues, keeping the cameras already found. Unrelated GVCP
  traffic arriving on the discovery socket — a foreign request id, a `READREG`
  ack, an error status, a runt datagram — is ignored rather than failing the
  call. On Windows a loopback probe could raise `WSAECONNRESET` and discard a
  perfectly good camera found on the wired NIC.
- **`Iface::from_ipv4` reports the address that was asked for.** It resolved
  the interface by name and then kept whichever IPv4 the OS listed last, so a
  multi-homed NIC — a stale DHCP lease alongside a link-local address — could
  bind to a different address than the caller selected.
- **The interface index is the kernel's, not a guess.** `Iface` now takes the
  index from `if_addrs`, which on Windows reports the adapter's real `IfIndex`.
  The previous Windows path returned the enumeration position + 1, which feeds
  multicast joins.
- **A truncated Discovery ACK no longer panics.** Reading past the end of a
  short payload hit `advance out of bounds`; every field after the IP address
  is now optional.
- **`=` and `<>` are the GenICam equality operators.** We required the C
  spellings `==` / `!=` and rejected `=` outright with "use `==` for equality".
  27 of 30 corpus documents contain at least one formula we therefore refused —
  531 of 679 in `Basler_acA1600_20gm` alone. `==` and `!=` remain accepted.
- **`<IntSwissKnife>` and `<IntConverter>` evaluate in integer arithmetic.**
  Everything ran through `f64`, so `(HIGH << 32) | LOW` lost its low bits and
  `(IDX / 2) * 4` did not truncate. A wide shift could also overflow and panic
  outright in a debug build.
- **A node's numeric type comes from its element name.** `<Output>` is not a
  GenApi element — it appears zero times across the corpus, while
  `<IntSwissKnife>` appears 3 125 times — so every integer knife on every real
  camera was modelled as a float.
- **Register addresses are the sum of their terms.** `<Address>`, `<pAddress>`
  and `<pIndex>` all contribute; we kept one and logged
  `ignoring fixed <Address> in favour of <pAddress>` for the rest. 42 registers
  on the reporter's camera — including the stream-channel registers — resolved
  to the wrong address, as did 454 on `FLIR_ORX_10G_51S5M` and 418 on
  `PGR_BlackflyS_13Y3M`.
- **`<pIndex Offset="N">` is now parsed** (197 occurrences across AVT,
  Prosilica and the GenICam conformance document). It was ignored, so every
  indexed register read the block base.
- **`<StructReg>` shares all of its address terms with its entries.** Only
  `<Address>` was read, and a `<StructReg>` without one defaulted to address 0:
  860 nodes on the reporter's camera, and 33 of 38 struct registers in
  `PGR_Blackfly_13E4C-CS`. A `<StructReg>` with no address at all is now
  reported instead of silently pointed at register zero.
- **Registers are unsigned unless `<Sign>Signed</Sign>` says otherwise.** We
  always sign-extended, so any register with its top bit set read negative — a
  `GevCurrentIPAddress` of 192.168.1.160 came back as `-1062731360`, and mask
  comparisons such as `(CTRL | 0xFDFFFFFF) = 0xFFFFFFFF` could never be true.
  `<StructReg>` entries also declared the full `i64` range, which made a set bit
  sign-extend to `-1` and quietly broke every `(INQ = 1)` test.
- **Converter reads use `<FormulaFrom>`, writes use `<FormulaTo>`.** The two
  were swapped. An identity converter behaves the same either way, which is how
  this survived; a `FROM * 100` / `TO / 100` pair read back wrong by 10⁴.
- **Converters can be written.** `<FormulaTo>` was parsed and never evaluated,
  so converter features were read-only. Writes now bind `FROM` to the incoming
  value and `OLD` to the register's current contents, so read-modify-write
  formulas such as `FROM | (OLD & 0xffff0000)` preserve the bits they should.
- **`<Constant>` and named `<Expression>` elements are supported**, resolved by
  substitution when the node is built.
- **`LG()` and the `E` / `PI` constants are supported.** `LG` is used by every
  Baumer TXG description for its dB scale.
- **`connect_gige` no longer panics on a malformed model** (backlog SR-01).
  `NodeMap::from` called `.expect()` on remote input.
- **An event could still be stranded behind the event channel's receive lock**
  (TC-14, Codex review on #69). The previous fix added a recheck of the queue
  after acquiring the receive lock, which closed the wide window but left a few
  instructions of a narrow one: the first event of a decoded datagram was
  returned and the rest queued *after* the lock was released, so a second
  consumer could take the lock, find the queue empty, and block in `recv_from`
  on a device that had already sent everything it was going to send. Every
  event a datagram carries is now published in one step while the lock is held,
  and the caller pops its own result from the queue like any other consumer —
  the first/remainder split that had to be ordered correctly is gone.
- **A node type we do not implement now says so** (GA-02). `is_node_tag` gated
  the parser's isolation path, so a tag not on its list fell through to
  `skip_element` and vanished — no log line, no `XmlModel::skipped` entry, and
  nothing for the corpus tests to trip over. `<Register>`, 56 declarations
  across 14 corpus documents, disappeared exactly this way. An element is now
  taken for a node declaration if it carries a `Name`, which the GenApi schema
  requires on every node and on nothing else at that level.
- **`NodeMap` no longer discards the XML layer's losses.** `try_from_xml` built
  its skip list from scratch and dropped `XmlModel::skipped` on the floor, so
  any consumer holding a nodemap — camctl, Python, Studio — could not tell a
  feature we failed to parse from one the camera does not have. The two lists
  now travel together.
- **Two concurrent `EventSocket::recv` callers could strand an event.** Both
  could observe an empty pending queue, after which one would decode a
  multi-event datagram, queue the remainder and return; the other was already
  committed to `recv_from` and never rechecked, so it blocked on a device that
  had finished speaking while its events sat in the queue. Found by codex
  review on #68.
- **The fake camera tracked event notification globally.** `EventNotification`
  is selected by `EventSelector`, so enabling two events writes one address
  twice — and a single stored word let the second write silently disable the
  first. State is now per event id. Found by codex review on #68.
- **Action commands were indistinguishable from register reads** (#61).
  `ACTION_CMD` was sent as opcode 0x0080 — which is `READREG_CMD`. A camera
  receiving one saw a register read with a 24-byte payload, so an action either
  failed or was interpreted as six register accesses. The opcode is 0x0100
  (`ACTION_ACK` 0x0101), and the payload is 12 bytes — `device_key`,
  `group_key`, `group_mask` — extended to 20 by a 64-bit action time only when
  the scheduled-action flag (bit 7 of the GVCP flags byte) is set. The old
  encoder always sent the time, plus a stream-channel field and a reserved word
  that the format does not have. `ActionParams::channel` is gone.
- **The GVCP event channel could not have worked against any camera** (#62).
  Four defects stacked on top of each other:
  - The parser required opcode 0x000D, which is not a GVCP opcode. Events
    arrive as `EVENT_CMD` (0x00C0) or `EVENTDATA_CMD` (0x00C2).
  - It decoded the datagram as an acknowledgement, reading the 0x42 command key
    and flags byte as a status word. Every field after that was shifted: the
    event identifier was read out of the reserved word, and the timestamp out
    of the stream channel and block id.
  - One `EVENT_CMD` may pack several events (`length / 16`, or `/ 24` with
    GigE Vision 2.0 extended block IDs). Only the first was considered.
  - No `EVENT_ACK`/`EVENTDATA_ACK` was ever returned, so a device that set the
    acknowledge-required flag would retransmit indefinitely.

  All four are fixed, and `EventPacket::block_id` widens to `u64` to carry
  extended block IDs.
- **Message-channel bootstrap registers pointed at nothing.** The destination
  address and port were written to 0x0900_0200 and 0x0900_0204. The real
  registers are `GevMCDA` at 0x0B10 and `GevMCP` at 0x0B00 — 0x0900 is
  `GevNumberOfMessageChannels`, and its value was being used as a base, so
  every write landed roughly 150 MB into the device's register space. The port
  was additionally written as a bare `u16` into a 32-bit register, placing it
  in the high half.

### Removed

- **The raw event-enable fallback**, which toggled a bit in a "notification
  mask" at 0x0900_0300. No such bootstrap register exists: which events a
  device emits is selected through the GenApi `EventSelector` and
  `EventNotification` features. A camera exposing neither now gets an error
  naming what is missing, instead of a write to an invented address.
  `GigeDevice::enable_event_raw` is gone with it.

### Changed

- **A node we cannot build costs that one feature, not the whole camera.** The
  per-node isolation added to the XML layer in 0.2.8 now extends to nodemap
  construction: failures are recorded in `NodeMap::skipped()` and logged rather
  than aborting. `connect_gige` logs one summary line naming how many features
  are unavailable.
- The vendor corpus test now has a second stage in `viva-genapi` that builds a
  `NodeMap` from each document and evaluates every node in it. Parsing was only
  ever half the job — every defect above lived above the parser, where the old
  test could not see it. All 35 documents now pass: 31 737 nodes.
- The corpus grew to 35 documents with four FLIR Blackfly / Blackfly S
  descriptions contributed by @themightyoarfish on #45. Two of them contain
  `SerialPortSelectorValueToIndex`, the `IntSwissKnife` whose `=` equality
  operator is what made their camera unopenable — so the regression fixture for
  that bug is now the vendor's own XML rather than our reconstruction of it.
- The in-tree fake camera's XML uses conformant GenICam spelling (`=`, `&lt;&gt;`,
  no `<Output>`), and exercises a summed `<Address>` + `<pIndex>` stream-channel
  register and a `<pAddress>`-addressed `<StructReg>` end to end.
- **The fake camera answers the mandatory bootstrap block.** `Version`,
  `DeviceMode`, the MAC registers, the IP configuration and the channel counts
  all read as zero before, so a register dump taken from the fake looked
  nothing like one taken from a camera. The MAC in the registers is now
  asserted against the MAC in the Discovery ACK — two copies of one fact
  drifting apart is how #57 happened.
- **The issue templates ask for artifacts GitHub will actually accept.** The
  camera template asked for a `.xml` attachment, which GitHub rejects, and put
  `render: shell` on the field where it told reporters to attach a file, which
  disables uploads (DOC-12, DOC-13 — both found by codex review on #58). It now
  asks for the `viva-camctl report` bundle.
- **ADR-0018 and ADR-0019 no longer treat aravis as an authority.** ADR-0018
  said to verify against "the reference implementation" and named `../aravis`
  the tiebreaker for formula semantics; ADR-0019 called aravis and Wireshark
  "the practical authorities". Both are amended. `CLAUDE.md` carries the
  ranking they now defer to: real hardware, then the specification, then the
  vendor XML corpus, then independent implementations as corroboration that is
  cited but never decisive. A question they cannot settle goes to the backlog
  to wait for a device (TC-09, TC-12).
- Both READMEs install with `cargo add viva-genicam` rather than a pinned
  `viva-genicam = "X.Y"` line. Nothing built that line, so it rotted to `"0.1"`
  and stayed there through all of 0.2; and a pin is wrong at one end or the
  other whenever the tree is ahead of what crates.io serves.

### Breaking

- `gige::DeviceInfo` gains `version`, `serial` and `user_name`, all parsed from
  the Discovery ACK (offsets 136, 216 and 232) and previously discarded. Use
  `DeviceInfo::from_ip` to build a record for a camera addressed directly by IP.
  Viva Studio now shows the camera's own serial number instead of its MAC.
- Reported MAC addresses shift by two bytes relative to 0.2.x — the old value
  was wrong. Anything keyed on it (a saved device list, a config file) needs
  updating.
- `Addressing::Fixed` and `Addressing::Indirect` are replaced by
  `Addressing::Sum { terms, len }` with the new `AddressTerm` and `IndexOffset`
  types. `Addressing::fixed()`, `byte_len()` and `referenced_nodes()` are
  provided as helpers.
- `impl From<XmlModel> for NodeMap` is replaced by `TryFrom`; use
  `NodeMap::try_from_xml`.
- `bytes_to_i64` and `i64_to_bytes` take a `Sign`.
- `NodeDecl::Integer` and `IntegerNode` gain a `sign` field; the SwissKnife and
  Converter declarations gain `bindings`.
- `viva_genapi::swissknife` is now public: `evaluate` takes an `EvalMode` and
  works in `Value` rather than `f64`.

## [0.2.8] - 2026-07-28

Second hotfix in the same family as 0.2.7: a single unusual node in a camera's
GenApi XML could make the whole camera unopenable. **Hikrobot users blocked on
#35 should upgrade** — and after this release, this class of problem degrades to
a missing feature rather than a failed connect.

### Fixed

- **Constant-formula SwissKnife nodes no longer abort the XML load** — a
  `<IntSwissKnife>` / `<SwissKnife>` whose `<Formula>` needs no inputs is legal
  GenICam and appears in the standard's own conformance documents, but we
  required at least one `<pVariable>` and failed the whole document without it.
  This blocked a Hikrobot MV-CS050-10GC on `PixelDynamicRangeMin_Value`
  (reported in #35 after the original fix landed).
- **Unrecognized `<AccessMode>` values no longer abort the XML load** — `R` and
  `W` (seen in third-party GenICam documents, including the standard's
  conformance fixtures) now map to `RO` / `WO`, `WR` maps to `RW`, and anything
  else falls back to `RW` with a warning instead of rejecting the document. `RW`
  is what an absent `<AccessMode>` already meant, and the device still enforces
  its own access rules.

### Changed

- **A node we cannot parse now costs that one feature, not the whole camera.**
  Every node element is parsed in isolation: the document reader consumes the
  element up front, so a parse failure can neither desync it nor abort the load.
  Failures are recorded in the new `XmlModel::skipped` (tag, `Name`, error) and
  logged at `warn`, so the loss is visible rather than silent. This is the
  structural fix behind #45, #35 and the two bugs above — all of them were a
  single odd node making an entire camera unopenable.

## [0.2.7] - 2026-07-28

Hotfix for a 0.2.6 regression that made cameras with certain vendor XML
impossible to open. **Anyone on 0.2.6 should upgrade.**

### Fixed

- **Cannot connect to FLIR (and other vendors') cameras: `unescape error: Cannot find ';' after '&'`** (#45).
  Connecting failed outright — `connect_gige` / `connect_u3v` returned an error and no
  camera handle — whenever the device's GenApi XML put a literal `&` somewhere it is
  perfectly legal, most commonly a `<![CDATA[...]]>` tooltip or a comment inside
  `<ToolTip>` / `<Description>`. Reported against a FLIR BFS-PGE-31S4C-C, but nothing
  about it is FLIR-specific.

  A regression from the quick-xml 0.31 → 0.41 migration in 0.2.6: the migration added
  entity unescaping (0.31 never unescaped at all, so `&amp;` used to leak through
  literally) but applied it to `Reader::read_text`'s **raw** span, which still contains
  markup. quick-xml documents that method as explicitly not unescaping, precisely because
  the span can contain CDATA.

  Text extraction is now an explicit event walk: character data is decoded, CDATA is taken
  literally, character and predefined entity references are resolved, and comments and
  processing instructions are dropped. An entity with no definition (GenICam declares no
  DTD, so e.g. `&copy;` has nothing to resolve against) is kept as written instead of
  failing the document.
- **A lone `&` in vendor XML no longer blocks connecting.** `parse` and
  `parse_into_minimal_nodes` enable quick-xml's `allow_dangling_amp`, so technically
  non-conformant XML — which we cannot fix and the camera will not stop shipping — loads
  with the `&` carried through verbatim. A cosmetic tooltip is never worth failing a
  camera connect over.
- **Malformed GenApi XML is now reported as a parse error, not a transport error.**
  `connect_gige` / `connect_u3v` mapped XML parse failures to `GenicamError::Transport`
  (Python: `TransportError`), which sent debugging off toward the network. They now return
  `GenicamError::Parse` (Python: `ParseError`) with the stage named in the message.
  **Note for Python users:** if you catch `viva_genicam.TransportError` around a connect
  call to handle bad XML, switch to `ParseError` (or the shared `GenicamError` base).

### Added

- **Regression coverage for vendor-XML text quirks** — parser tests for CDATA, a dangling
  `&`, comments inside text elements, entity references and the whitespace around them,
  numeric character references, and undefined entities. The fake GigE camera's `Gain`
  tooltip is now a CDATA section containing `&` and `<`, as several vendors ship, so
  `test_connect_with_cdata_tooltip` exercises the fix through a full end-to-end connect.

## [0.2.6] - 2026-07-25

### Fixed

- **Every GVSP frame was reassembled at the wrong stride, and short frames were delivered as if complete** ([#34](https://github.com/VitalyVorobyev/viva-genicam/pull/34), thanks [@Katze719](https://github.com/Katze719)). The per-packet payload stride is `GevSCPSPacketSize` minus 36 bytes of overhead (IP 20 + UDP 8 + GVSP 8), not minus 8, so every packet after the first was written to the wrong offset; the expected-packet count was off by one for payloads that divide evenly; and `is_complete()` was never consulted, so a frame missing packets was handed to the caller as a valid image rather than dropped. *(Entry added retroactively on 2026-07-31: this shipped in 0.2.6 — `69c6c80` is an ancestor of `v0.2.6` — but was omitted from the changelog at the time.)*
- Cap decompressed GenICam XML at 64 MiB to prevent memory exhaustion from malicious or corrupt ZIP metadata served by a device.
- **GigE connect fails on Hikrobot cameras with `InvalidParameter`** (#35) -- two independent fixes:
  - `read_mem` now rounds every READMEM byte count up to a multiple of 4 as GVCP requires and drops the padding; strict cameras (Hikrobot) reject unaligned counts, which broke the final partial block of the XML download.
  - `fetch_and_load_xml` transparently decompresses ZIP-packed GenApi XML (`PK\x03\x04` magic), which Hikrobot/Basler/FLIR cameras commonly serve, and falls back to `GevSecondURL` (0x0400) when the first URL register is empty or unreadable.

### Added

- **Fake GigE camera realism** -- the fake GVCP server now rejects unaligned READMEM requests with `GEV_STATUS_INVALID_PARAMETER` exactly like strict real cameras (regression guard for #35), and `FakeCameraBuilder::zip_xml(true)` serves the GenApi XML as a ZIP archive. New integration test `test_connect_with_zipped_xml` covers the zipped + unaligned-length connect path end-to-end.

### Changed

- **quick-xml 0.31 → 0.41** -- clears RUSTSEC-2026-0194 (quadratic duplicate-attribute check) and RUSTSEC-2026-0195 (unbounded `NsReader` namespace allocation). API migration: `Reader::trim_text` → `config_mut().trim_text(true)`, `Attribute::unescape_value` → `normalized_value(XmlVersion::Implicit1_0)`, `read_text` now returns `BytesText` (decode + unescape explicitly).
- **Dependency refresh** -- full `cargo update` to the latest semver-compatible versions after three dormant months (139 lock entries), which also clears the advisories against anyhow (RUSTSEC-2026-0190 unsound `downcast_mut`), crossbeam-epoch (RUSTSEC-2026-0204), quinn-proto (RUSTSEC-2026-0185), and rustls-webpki (RUSTSEC-2026-0104), and replaces yanked spin/stabby releases. `cargo deny check` is fully green again.
- **License checking enforced** -- `deny.toml` gains `[licenses]` (permissive allow-list, MPL-2.0 exception for `option-ext`, documented libusb1-sys vendored-LGPL caveat), `[bans]`, and `[sources]` sections; CI and the weekly audit now run the full `cargo deny check`. The weekly audit workflow switches from cargo-audit to cargo-deny so `deny.toml` is the single source of truth for accepted advisories.
- **PyPI license metadata corrected for vendored libusb** -- the binary wheel statically links libusb 1.0.27 (LGPL-2.1-or-later) via `rusb`'s `vendored` feature, but the package metadata claimed plain MIT. The license expression is now `MIT AND LGPL-2.1-or-later`, the LGPLv2+ trove classifier is added, and the wheel/sdist ship `THIRD-PARTY-NOTICES.md` plus the full LGPL-2.1 text (`LICENSES/LGPL-2.1.txt`), including LGPL §6 relinking instructions (rebuild from source against a system libusb with the vendored feature disabled).

## [0.2.5] - 2026-04-15

### Changed

- **PyO3 / NumPy 0.22 → 0.28** -- port the Python bindings to current PyO3: drop `_bound`-suffixed constructors (`PyDict::new_bound`, `PyBytes::new_bound`, `PyArray1::from_slice_bound`, `PyModule::new_bound`, `py.import_bound`, `py.get_type_bound`, `from_vec_bound`, `empty_bound`), migrate `Python::allow_threads` to `Python::detach`, replace the removed `PyObject` type alias with `Py<PyAny>`, opt in to `FromPyObject` on `Clone + pyclass` types explicitly, handle `PyList::new`'s new `PyResult` return type. Wheel rebuilds cleanly with zero warnings.

### Fixed

- **Logo renders identically on every platform** -- the SVG at the top of the README loaded Nunito Black and Fira Code from Google Fonts via `@import`, which GitHub's SVG sanitizer strips. On Windows the fallback font shifted "viva" so it overlapped "genicam" and misplaced the red dot. Flatten all text to SVG paths (instanced the variable Nunito font at weight 900) so the logo is font-independent, and re-center the dot over the dotless `ı`.
- **EADDRINUSE race in the fake GigE camera** -- `FakeCamera::stop` used to fire `JoinHandle::abort()` and return immediately; the tokio task still held an `Arc<UdpSocket>` clone briefly, so rebinding the same port back-to-back (e.g. between module-scoped pytest fixtures) intermittently failed on macOS-14 / Python 3.9. `stop` is now `async` and awaits both join handles after aborting; the Python binding drives it via the shared runtime. Also set `SO_REUSEPORT` on the GVCP socket as defense-in-depth since `SO_REUSEADDR` is a UDP no-op on macOS.
- **CI build failure for `viva-fake-gige`** -- `socket2::set_reuse_port` sits behind socket2's `all` feature; local builds picked it up via transitive feature unification with `viva-gige`, but clean CI builds that don't pull in `viva-gige` failed with `no method set_reuse_port found`. Enable `socket2 = { features = ["all"] }` on `viva-fake-gige` directly.

### Added

- **`CLAUDE.md` pre-push checklist** -- document the three local gates CI runs with warnings-as-errors (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo doc --workspace --all-features --no-deps`) and the version-bump procedure (root `Cargo.toml`, `viva-pygenicam/Cargo.toml`, `viva-pygenicam/pyproject.toml`, `CHANGELOG.md`). Also require verifying the latest crate version on crates.io before any dependency bump.

## [0.2.4] - 2026-04-13

### Added

- **Python bindings (`viva-genicam` on PyPI)** -- new `crates/viva-pygenicam` PyO3 crate plus pure-Python facade in `python/viva_genicam/`. Ships as an abi3 wheel covering discovery, control, introspection, and streaming for both GigE Vision and USB3 Vision cameras. Frames expose a NumPy-friendly `to_numpy()` / `to_rgb8()` API, streams are sync iterators over a managed Tokio runtime (no asyncio required), errors map onto a `GenicamError` subclass hierarchy, and `py.typed` + `.pyi` stubs give IDEs full completion. Fake-camera pytest suite (19 tests) runs against the built wheel.
- **Python wheels CI (`.github/workflows/python.yml`)** -- cross-platform wheel matrix (Linux x86_64, macOS arm64, Windows x86_64) × Python 3.9–3.13; libusb is statically vendored into the extension so no system libusb is required by the wheel. Publishes to PyPI via OIDC on `py-v*` tags. Test install uses `pip --no-index --find-links dist` so CI never falls back to a published PyPI wheel while validating the just-built artifact.
- **Auto-detected GigE streaming interface** -- `camera.stream()` now picks the NIC whose IPv4 subnet contains the camera's IP when `iface` is omitted; loopback cameras resolve to `lo`/`lo0` automatically. An explicit `iface=` override remains available on both `connect_gige` and `stream`.
- **`book/src/python.md`** -- Python API tutorial chapter; README gains a Python section.
- **Python examples** -- `crates/viva-pygenicam/examples/` ships five runnable scripts (`discover.py`, `get_set_feature.py`, `node_browser.py`, `grab_frame.py`, `demo_fake_camera.py`) plus an `examples/README.md` that describes each.
- **Expanded book tutorial** -- `book/src/python.md` becomes an index; five sibling pages under `book/src/python/` walk through install, discovery, control & introspection, streaming, and a full API reference.
- **In-process fake camera (`viva_genicam.testing.FakeGigeCamera`)** -- the `viva-fake-gige` crate is now bound as a Python class shipped inside the wheel. `pip install viva-genicam` alone is enough to run the full demo end-to-end; no subprocess, no binary to build, no repo clone required. The `demo_fake_camera.py` example and `tests/conftest.py` were migrated onto the in-process path.

### Changed

- Root `Cargo.toml` gains `workspace.exclude = ["crates/viva-pygenicam"]` so PyO3/maturin stays out of the default `cargo test --workspace` path.

## [0.2.3] - 2026-04-12

### Added

- **Zenoh API v2 `FeatureState` contract** -- new `FeatureState`, `NumericRange`, and `CommandResult` wire types expose live feature introspection (is_implemented / is_available / access_mode / numeric range / enum_available) per node; `introspect` queryable wired into `viva-service` and `viva-service-u3v`. Legacy `NodeValueUpdate` stays wire-compatible. See ADR-010.
- **GenApi predicate evaluation** -- `NodeMap::is_implemented`, `is_available`, `effective_access_mode`, and `available_enum_entries` evaluate `pIsImplemented` / `pIsAvailable` / `pIsLocked` / per-enum-entry predicates via the existing `resolve_numeric` machinery (Integer / Boolean / SwissKnife / IntConverter / Converter / Enum providers all supported, with cycle detection)
- **Predicate refs on every `NodeDecl` variant** -- `PredicateRefs` (`p_is_implemented` / `p_is_available` / `p_is_locked`) parsed from `<pIsImplemented>` / `<pIsAvailable>` / `<pIsLocked>` and plumbed through `NodeMap::try_from_xml` with proper dependency registration; `Node::predicates()` exposes the refs for external evaluators
- **Realistic predicate wiring in the fake GigE camera** -- `ExposureTime.pIsLocked` ← `ExposureAuto != Off`, `Gain.pIsLocked` ← `GainAuto != Off`, `AcquisitionFrameRate.pIsAvailable` ← new `AcquisitionFrameRateEnable` Boolean, `PixelFormat` entries gated by a new `SensorType` enum (Monochrome / BayerRG / Color)

### Changed

- **`DeviceHandle::get_feature_state`** now reports live `is_implemented` / `is_available` / `access_mode` / `enum_available` from the predicate evaluators instead of hardcoded permissive defaults; each predicate call is guarded so a single bad formula doesn't break the whole feature snapshot

### Fixed

- **Float bit-pattern bug** -- `<FloatReg>` and bare `<Float>` with `Length in {4, 8}` and no `<Scale>`/`<Offset>` now auto-infer IEEE 754 encoding. Before this fix, `AcquisitionFrameRate` came back as `1106247680` (the f32 bit pattern of 30.0) and `ExposureTime` as `4662219572839973000` because float registers were always read as scaled i64. New `FloatEncoding` (Ieee754 / ScaledInteger) + `byte_order` on `NodeDecl::Float`; `get_float` / `set_float` dispatch on encoding.

## [0.2.2] - 2026-04-12

### Changed

- Rename old repo name `viva-genicam` to the new `viva-genicam`

## [0.2.1] - 2026-04-12

### Added

- **Multi-platform release binaries** -- release workflow now produces prebuilt `viva-camctl` and `viva-service` archives for Linux x86_64, macOS aarch64 (Apple Silicon), and Windows x86_64; each archive bundles the binaries with `README.md`, `LICENSE`, and `CHANGELOG.md`, and a `SHA256SUMS.txt` is published alongside
- **`viva-camctl` on crates.io** -- the CLI is now published, so `cargo install viva-camctl` works

### Changed

- Internal workspace dependency version requirements simplified from `"0.2.0"` to `"0.2"` (semver-equivalent, but avoids a sweep on every patch bump)
- Release workflow dropped the redundant `.crate` packaging step -- those archives are hosted on crates.io via the publish-crates workflow

### Fixed

- `GigeRegisterIo` now detects async context via `Handle::try_current()` and wraps `block_on` in `tokio::task::block_in_place` only when inside a runtime, preventing nested-runtime panics while preserving plain synchronous usage

## [0.2.0] - 2026-04-11

### Added

- **USB3 Vision streaming** -- `U3vFrameStream` async frame iterator wrapping blocking bulk reads via `spawn_blocking`, `U3vStreamBuilder` for configuring U3V streams through the same pattern as GigE
- **USB3 Vision service** -- `viva-service-u3v` now supports real USB cameras (previously `--fake` only); `U3vDeviceHandle` is generic over `T: UsbTransfer`
- **USB3 Vision CLI** -- `viva-camctl stream-usb` command for frame streaming from USB3 Vision cameras
- **FORCEIP command** -- GVCP opcode 0x0004 for temporary IP assignment via broadcast (targets device by MAC address)
- **Persistent IP configuration** -- read/write bootstrap registers for persistent IP, subnet, and gateway; `enable_persistent_ip()` method on `GigeDevice`
- **IP configuration CLI** -- `viva-camctl set-ip` command with `--force` (FORCEIP) and persistent register modes
- **Reconnection with backoff** -- `DeviceHandle::refresh_connection()` retries up to 5 times with exponential backoff (500ms base, 16s max)
- **GenApi node metadata** -- `NodeMeta` struct with `Visibility`, `Description`, `ToolTip`, `DisplayName`, `Representation` fields; parsed from XML and exposed on all node types
- **Visibility filtering** -- `Visibility` enum (Beginner/Expert/Guru/Invisible), `Representation` enum (Linear/Logarithmic/HexNumber/etc.), `NodeMap::nodes_at_visibility()` for UI filtering
- **`U3vDevice::transport()`** -- public accessor for the shared USB transport `Arc<T>`
- **Bayer 16-bit pixel formats** -- `PixelFormat` enum now includes BayerGR16, BayerRG16, BayerGB16, BayerBG16 with correct PFNC codes; `PixelFormat::from_name()` for string-to-enum conversion
- **`PixelFormat::from_name()`** -- parse PFNC name strings (e.g. "RGB8", "Mono16", "BayerRG16") to `PixelFormat`

### Changed

- MSRV raised from 1.85 to 1.88 (resolves `time` crate security advisory RUSTSEC-2026-0009)
- Project tagline updated from "Ethernet-first" to "GigE Vision and USB3 Vision" reflecting dual-transport support
- `viva-service-u3v` Cargo.toml now enables `u3v-usb` feature for real USB support
- Added `cargo deny check advisories` to CI pipeline with `deny.toml` allow-list for zenoh transitive advisories

### Fixed

- GitHub Pages deployment error ("Tag v0.1.0 not allowed to deploy") by removing wildcard tag trigger from `publish-docs.yml`
- SVG logo dot alignment and genicam text spacing for correct browser rendering
- `time` crate DoS vulnerability (RUSTSEC-2026-0009) by upgrading to 0.3.47

### Known Issues

- `lz4_flex 0.10.0` (RUSTSEC-2026-0041, high) and `rsa 0.9.10` (RUSTSEC-2023-0071, medium) are transitive dependencies through `zenoh 1.9.0` and cannot be updated until zenoh releases a fix. Neither is exploitable through our usage (lz4 decompression of untrusted data, RSA timing attack). Tracked in `deny.toml`.

## [0.1.0] - 2026-04-10

Initial public release of the viva-genicam workspace.

### Added

- **viva-genicam** -- High-level facade crate with `Camera<T>`, discovery, streaming, events, and action commands
- **viva-gige** -- GigE Vision transport layer: GVCP discovery, GenCP register I/O, GVSP streaming with resend and reassembly
- **viva-genapi** -- In-memory GenApi node map with typed feature access (Integer, Float, Enum, Boolean, Command, SwissKnife, Converter, String)
- **viva-genapi-xml** -- GenICam XML parsing into an intermediate representation with async XML fetch
- **viva-gencp** -- Transport-agnostic GenCP protocol encode/decode
- **viva-u3v** -- USB3 Vision transport: bootstrap registers, GenCP-over-USB control, and bulk streaming
- **viva-pfnc** -- Pixel Format Naming Convention (PFNC) tables and helpers
- **viva-sfnc** -- Standard Feature Naming Convention (SFNC) string constants
- **viva-zenoh-api** -- Shared Zenoh API payload types (no Zenoh dependency)
- **viva-service** -- Zenoh bridge exposing GenICam cameras as network services

### Protocol Features

- GVCP discovery (broadcast and unicast)
- GenCP register read/write with retry and backoff
- GVSP streaming with frame reassembly
- Packet resend with bitmap tracking and exponential backoff
- Automatic packet size negotiation from MTU
- Multicast stream support (IGMP join/leave)
- GVCP event channel with timestamp mapping
- Action commands with scheduled execution
- Chunk data parsing (timestamp, exposure time, gain, line status)
- Extended ID support (64-bit block IDs, 32-bit packet IDs per GigE Vision 2.0+)

### Testing

- `viva-fake-gige` -- In-process fake GigE Vision camera for self-contained integration testing (no external dependencies required)
- `viva-fake-u3v` -- In-process fake USB3 Vision camera for testing

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.6.0
[0.5.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.5.0
[0.4.1]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.4.1
[0.4.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.4.0
[0.3.1]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.3.1
[0.3.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.3.0
[0.2.8]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.8
[0.2.7]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.7
[0.2.6]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.6
[0.2.5]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.5
[0.2.4]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.4
[0.2.3]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.3
[0.2.2]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.2
[0.2.1]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.1
[0.2.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.0
[0.1.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.1.0
