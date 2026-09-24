# Roadmap

Mid-term direction, ordered by phase. **This file only looks forward.** Shipped
history lives in [CHANGELOG.md](../CHANGELOG.md), and immediate actionable tasks
in [backlog.md](backlog.md). It used to carry a closed phase per release, which
made it a second changelog and a worse one; a phase is deleted when it closes.

**Ordering principle.** [ADR-0018](adrs/adr0018-genapi-conformance-over-convenience.md)
established that priority is argued from measurement, not intuition: count the
construct in the vendor corpus, or point at the user report, before ranking it.
The phases below are numbered by the order they were opened, not by the order
they will close — the evidence hierarchy in
[CLAUDE.md](../CLAUDE.md#evidence-hierarchy) decides that, and it routinely
promotes something out of a later phase because a user with hardware appeared.

## Phase 1 — Transport conformance (ADR-0019)

ADR-0018 audited the GenApi layer against the specification and found eight
defects. The same audit had never been run on GVCP/GVSP, and the wire layer
carried the same class of error: a `PENDING_ACK` nobody handled, an
`ACTION_COMMAND` sharing an opcode with `READREG`, an event channel keyed on a
number that is not a GVCP opcode, and a GVSP trailer read at the wrong offset.
Those are fixed and recorded in [CHANGELOG.md](../CHANGELOG.md); the pattern is
what this phase is still about.

**Open**: TC-05 (the payload types cameras actually send), TC-06 (chunk trailer
layout), TC-12 (the `PENDING_ACK` field width, unsettled against hardware),
TC-16 (per-transport status-code types).

**The structural half of this phase matters more than any single fix.**
Issue #57's MAC offset is the *third* time the fake camera and the client have
shared an identical wrong assumption, after the SCPS overhead and the unaligned
READMEM (see [design.md](design.md#testing-strategy)). The realism policy says
fakes must be derived from the spec; nothing enforces it. ADR-0019 adds the
enforcement: **fake-camera wire fixtures are spec-derived byte arrays, asserted
independently of the client parser**, so producer and consumer can no longer
agree on a shared error.

## Phase 2 — Diagnostics loop

The binding constraint on this project is that the maintainer has no hardware:
every fix so far has been diagnosed from an artifact a user supplied. #35 was
solved by a model dump, #45 by the byte offsets in a traceback, #57 by a
reporter who read the Wireshark dissector. Yet there is no supported way to
produce those artifacts — `viva-camctl` has no XML dump, and the Python
retrieval snippet given in #45 had to be retracted and corrected.

`viva-camctl xml` and `viva-camctl report` now close that gap — both work on a
camera we cannot open, which is the only camera anyone reports — and discovery
parses the serial and user-defined name it used to discard.

**Open**: DX-05 (skipped nodes reach camctl but not Python or Studio), DX-06 (no
`report` equivalent for USB3 Vision).

The loop keeps paying out, and not only through bug reports: **two defects have
been found by reading a log a reporter attached for an unrelated reason**, and
one of them was our own diagnostic instruction failing on the first person we
gave it to. That is the argument for making the artifacts easy to produce even
when nothing is known to be wrong.

## Phase 3 — Streaming reliability

The features that make the library trustworthy on a factory floor.

- **Per-stream ephemeral ports + `source_filter` enforcement** — the filter is
  configured today but never applied, because the receive path discards the
  packet's source address.
- **Wire packet resend end-to-end** — `ResendPlanner` and `request_resend`
  exist, are tested, and have no production callers. Either wire them or delete
  them; the README currently advertises them as shipping.
- **Honest streaming telemetry** — five `StreamStats` counters are permanently
  zero because nothing calls their recorders, and every GVSP parse error is
  swallowed at `trace` level and counted nowhere.
- **IGMP leave on multicast teardown** — the group is joined and never left.
- **The per-packet `HashSet` on the receive hot path** — one insert per datagram,
  roughly 2 100 hashes for a 3.1 MB frame at a 1 500-byte packet size.

## Phase 4 — GenApi conformance, round 2

What ADR-0018 did not reach, ordered by corpus frequency rather than by how
interesting it looks. **The counts live in `backlog.md` and are not repeated
here**, because they were measured against a corpus that keeps growing and every
copy of a number is one more place for it to go stale. Re-measure before quoting
one, with a whole-element match: a line-based `grep` cannot count elements in the
single-line XML that FLIR and PGR ship, which is how `<Register>`'s count came to
be wrong by seven declarations and its `<pLength>` split wrong by a factor of
eight.

**The two that were at the front of this phase were not from the corpus at
all.** The integer-codec defects came from users' cameras rather than from
reading XML, and they have shipped — see the top of this file.

- `pInvalidator` — **18 502 occurrences across 32 of 35 documents**, entirely
  unparsed. Cache invalidation currently fires only on writes made through the
  NodeMap.
- `Cachable` (2 735 / 32) and `PollingTime` (327 / 28) — unparsed, so every
  readable node is cached until a dependency is written.
- `pSelected` (1 534 / 31) — parsed with the direction inverted relative to the
  standard, which registers invalidation edges backwards.
- `pMin` / `pMax` (911 / 1 288) — parsed, stored, registered as dependencies, and
  never read; range checks use the static limits.
- `ImposedAccessMode` (2 709 / 28), `Streamable` (1 700 / 18), `Slope`
  (632 / 29), `pInc` (333 / 25) — unparsed.
- `<Register>` — the raw-byte base register type, still partly dropped. No
  longer dropped *silently*: unknown node tags go into `XmlModel::skipped`, so
  the corpus allowlist sees them. Two vendors' hardware and an outside contributor's API request all
  point at the same node, `FileAccessBuffer`. Taking the 42 plain-`<Length>`
  declarations first leaves only 21, concentrated in three vendors.
- GenApi chunk adapter, to replace the hardcoded 4-entry chunk table.

**Also in scope: finish making the corpus test able to fail.** Its
`viva-genapi` stage now evaluates each document twice — against `NullIo` and
against a descending byte pattern that sets the top bit in either byte order —
which is what the integer-codec defects needed to be caught. It still asserts no
*values*, so it demonstrates that nothing errors rather than that anything is
right, and a byte-order regression would pass it. Per-document value
expectations are what remains of GA-11.

## Not a phase — device classes beyond area-scan

Every assumption in this codebase is an area-scan camera's. On 2026-07-31 a
Micro-Epsilon scanCONTROL 850050 — a laser profile scanner — became the first
non-area-scan device in the corpus, contributed by the same person who added
its `Coord3D_*` pixel formats. The formats now exist in `viva-pfnc`; the device
class does not exist anywhere above it. A `Coord3D_ABC32f` frame reaching the
Zenoh bridge today is truncated to a twelfth of itself and published as valid.

**Event cameras are the second class, and the first one somebody is actually
streaming.** [#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138) and [#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139) bring a Lucid Triton2 EVS on a
Sony IMX636: a GigE Vision camera that publishes no `PixelFormat` in its GenApi
XML, sends a vendor-defined code with PFNC's custom bit set, declares
`payload_type` 0x0001 as if it were an image, and emits an encoded event stream
rather than a frame. So the reassembly path already works on it — what does not
work is everything that assumes the reassembled bytes are pixels. The contributor
read the wire with a capture, which is precisely the evidence this section says it
is waiting for.

The open question was not how to parse the block; it was **which topic a non-image
stream belongs on**, because `viva-service` published one as a 1×64000 image with
an unnamed format. That was ours rather than the contributor's to settle, and it
is settled: event blocks go on their own `evs` topic behind an `EvsHeader`, never
on `image` (Zenoh API version 4). What remains of DC-05 is a Python `next_block`
and a test for the Windows receiver.

This is deliberately not a numbered phase. The concrete defects are tracked in
`backlog.md`'s `DC` section, and only the ones verified against code rather than
inferred about hardware are scheduled. The rest wait for the thing the evidence
hierarchy actually values: somebody streaming one of these devices and telling us
what came off the wire.

## Phase 5 — API consolidation (breaking)

One deliberate breaking release to pay down surface-area debt. It has been
renumbered twice, because both 0.4.0 and 0.5.0 turned out to owe their breaking
window to a user's camera instead. That is the right trade every time, and it is
also why this phase should stop being described by a version number: it lands in
whichever breaking release is not already spoken for.

- Typed accessors on `Camera`. Everything currently round-trips through
  `String` even though `NodeMap` one layer down already has
  `get_integer`/`get_float`/`get_bool`/`get_enum`.
- Type-gate the GigE-only methods. `configure_events` and
  `configure_stream_multicast` write GVCP bootstrap registers but are defined
  on the generic `Camera<T>`, so they compile against a U3V camera.
- Single frame-reassembly implementation shared by all paths.
- Curated public surfaces: kill blanket `pub mod`; re-export currently
  unnameable public types; stop exposing node cache internals.
- Error source chains everywhere (no `String` payloads). The
  `#[non_exhaustive]` half of this landed in 0.4.0 for the five enums that
  grow; what remains is deciding the policy for enums added after it.
- Dedupe viva-service vs viva-service-u3v behind a `StreamSource` trait.
- Fakes import register constants from the transport crates.
- viva-pfnc as the single `PixelFormat` authority.
- Workspace lints: `missing_docs`, `unreachable_pub`.

## Phase 6 — Production infrastructure

- **Per-crate feature matrix in CI.** `viva-pygenicam` and `studio` are
  excluded from the root workspace, so `cargo test --workspace` never builds
  them; and because sibling crates enable `u3v-usb`, feature unification means
  `viva-genicam`'s *own* default feature set is never verified.
- MSRV job; Windows wheels are published but only tested on Linux and macOS.
- Fuzzing for packet and XML parsers (untrusted network input).
- cargo-semver-checks on release tags.
- Book: production-tuning chapter (rmem, udev, usbfs).
- **Documentation accuracy.** Several book chapters document APIs that never
  existed in this codebase, and the README advertises resend and backpressure
  that have no production callers. This is wrong documentation rather than
  missing documentation, and is tracked as such.

## Services & Studio

- **Announce cadence exceeds Studio's expiry window** — the GigE service
  re-announces roughly every 7 s against a 6 s expiry, so devices can flicker.
- **U3V service streaming never configures the SIRM** or enables streaming.
- **M10** — real-service integration (studio against viva-service, not mocks).
- **M11** — release prep: DMG/AppImage/MSI packaging, service sidecar.
- **M12** — recording & polish.
