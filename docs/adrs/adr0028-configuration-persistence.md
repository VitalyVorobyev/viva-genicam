# ADR-0028: Saving and restoring a configuration in GenApi's feature-file layout

**Status:** Accepted; amended 2026-10-13 (selectors that cannot be set)
**Date:** 2026-10-11

## Context

GenICam Standard v2.1.1 §2.8.1 gives every value node a `<Streamable>`
element: it "denotes that the corresponding feature is prepared to be stored
to and loaded from a file via the GenApi node tree. The idea is to persist the
state of a camera by storing the features marked as Streamable and restore the
state by writing those features back to the node tree." The default is `No`
(§3.2). 20 of the 38 corpus documents use it, 1 805 times: 743 enumerations,
598 integers, 324 booleans, 109 floats, 16 string registers, 7 registers, 6
integer registers, 2 converters. We parsed none of it, and offered no way to
save a configuration (backlog GA-14).

Three things had to be decided: what a saved configuration holds, the order a
restore writes it in, and the file.

**What the standard says, and does not.** The standard defines the flag and
the intent, and no file format. It says nothing about selectors, the order of
writes, or what to do with a feature that cannot be written.

**What GenApi's reference implementation writes.** The EMVA reference
implementation's `Persistence.h` (the `CFeatureBag` class, EMVA GenICam
licence; read from the copy roboception's `rc_genicam_api` vendors) writes a
text file:

```text
# {05D8C294-F295-4dfb-9D01-096BD04049F4}
# GenApi persistence file (version <major>.<minor>.<subminor>)
Width<TAB>640
Gain<TAB>{GainSelector=DigitalAll}<TAB>9
```

The magic GUID must be on the first line; `#` lines are comments; a value is
`Name<TAB>Value`, and a value read under selectors puts them in braces between
the two, `Selector=Entry`, tab-separated. Its reader refuses a line with no
value and a file of the other format, `CFeatureBagger`. `LoadFromBag` collects
an error per failed write and continues. Tools built on the reference
implementation's `CFeatureBag` read and write this layout. Whether pylon's
`.pfs` files, or any vendor tool's, are byte for byte this layout we have not
seen: nobody has sent us one.

**The corpus.** Selectors whose `<pSelected>` names a streamable feature: 102
enumerations, 18 integers — `LUTIndex`, `GevStreamChannelSelector`,
Hikrobot's `HDRSelector`. Many selectors are not themselves streamable. The
dependencies between streamable features include cycles: `Width` and
`OffsetX` bound each other through `<pMax>`.

## Decision

**The file is the reference implementation's layout.** It is the only
persistence format the GenICam ecosystem has, so a file we write can be read
by a GenApi-based tool and theirs by us; it is line-oriented text, so it
diffs; and every value is the feature's own text, so it round-trips. A TOML or
JSON format of our own would be no more expressive and would interoperate with
nothing.

- First line `# {05D8C294-F295-4dfb-9D01-096BD04049F4}`; then `#` comments
  naming the camera — `VendorName`, `ModelName`, `DeviceXmlVersion` (the XML
  file's `MajorVersion.MinorVersion.SubMinorVersion`, §2.7) — and one
  `# not saved: Feature[Selector=Entry]: reason` per value left out. The
  reference reader skips them all.
- `Name<TAB>Value`, and `Name<TAB>{Selector=Entry<TAB>…}<TAB>Value` for a
  value read under selectors, outermost selector first.
- Values as text: integers in decimal (`0x` accepted on reading), floats in
  the shortest form that reads back the same, enumeration entries by name,
  booleans `1`/`0` (`true`/`false` accepted), a `<Register>` as `0x` and its
  bytes in hexadecimal. An empty value is left out, with a comment, because
  the reference reader refuses one.
- The reader also takes plain lines that set a selector and then a selected
  feature, as a replay of writes, and binds each value to the selector line
  before it.

**What a save holds.** Every node marked `<Streamable>Yes</Streamable>` except
categories and commands, in restore order. A feature behind selectors is read
under every combination of their entries — an enumeration's available entries,
an integer's values from minimum to maximum by increment — and each selector
is put back. Its selectors are those that select it and, in turn, theirs,
outermost first: `LUTValue` behind `LUTIndex` behind `LUTSelector` is read
under every `LUTSelector` entry, though `LUTSelector` does not name it (added
in milestone K, after a Codex review of #200). More than 65 536 combinations
leaves the feature out, so a save always ends. A feature is left out, with the reason, when it cannot be both
read and written (declared `RO` or `WO`), or is not implemented, not
available, or locked now. The empty `<Streamable/>` Basler's acA1600 declares
once is `No`: the schema's values are `Yes` and `No`.

**The order of a restore** comes from the dependency graph the nodemap already
keeps for cache invalidation. A feature is written after every streamable
feature whose write can change it — anything that graph reaches from the nodes
a write goes through: a selector, a `pIsAvailable`/`pIsLocked` provider, a
`<pMin>`/`<pMax>`, an address term, a `<pInvalidator>`. Features in a cycle are
written in document order, and the plan follows document order otherwise. The
file's order does not matter.

**A failed write is tried once more**, after all the others: only the order
can make `Width` wait for `OffsetX`. What fails then is reported with its
reason, in the file's order, and nothing stops the load early — as
`LoadFromBag` does. A configuration from another `VendorName`/`ModelName`, or
another major version of the XML ("not backward compatible", §2.7), loads
with a warning.

**`TLParamsLocked` is left alone.** Nothing in the standard asks a restore to
set it. A camera that is acquiring locks what it locks, and those writes are
reported; a save while acquiring leaves them out, with the reason.

**The layering.** What to save and in which order, the walk over selectors,
the writes and the text format are in `viva-genapi` (`persistence`), which
touches no file and builds for wasm. `Camera::save_features(&mut impl Write)`
and `load_features(&mut impl Read)` do the I/O.

## Consequences

### Positive

- One call saves and restores a camera, and the file is readable and
  diffable. `viva-camctl save`/`load`, Python and Rust share it.
- The vendor corpus test plans every document's save, checks the order against
  dependencies found independently of the plan, and saves against memory to
  check that every selector is put back.

### Negative

- Interoperability with vendor tools is argued from the reference
  implementation's source, not from a vendor's file. A user's `.pfs` or
  Spinnaker file would settle it (backlog GA-46).
- A save walks every selector entry, one register read at a time: a 4 096-entry
  LUT is 4 096 reads.
- A save sets selectors to entries it then puts back. A device that acts on a
  selector write — rather than only redirecting the selected feature — would
  see those writes.
- Locked features are left out of a save. A camera that locks a feature for a
  reason that has nothing to do with its own configuration loses it from the
  file.

## Amendment — 2026-10-13: a selector that cannot be set selects nothing

**Found on hardware.** On a MATRIX VISION mvBlueFOX3-2004G, the first camera
to run `save`, `ExposureTimeSelector` is not implemented (its
`<pIsImplemented>` reads 0) and still names `ExposureTime` in `<pSelected>`.
The walk tried to set it to each entry, every write failed with "node
unavailable", and `ExposureTime` — the feature every configuration is for —
was left out of every save, with nine other features behind the two other
selectors that camera does not offer now (`mvAreaSelector`,
`mvStepperMotorSelector`). Ten selectors that never moved were then "put
back", which failed and warned 24 times.

**Decision.** A selector selects only while it can be set: declared
writable, implemented, available and not locked now. One that cannot leaves
each feature it selects a single value, which the walk saves once, without
that selector in its group, and a load writes back without moving it. A
selector the walk could set is put back only if a write moved it; a load sets
a selector only when it does not already hold the value's entry.

**Evidence.** GenApi's reference implementation does this walk in
`CSelectorSet` (`GenApi/SelectorSet.h`): its headers, which
`rc_genicam_api` republishes, say only that it "recursively explores the
selectors and sub selectors of a node" and that `Restore` "restores the
selectors' values found at creation"; its source (`SelectorSet.cpp`) is not
public, so whether it skips an unavailable selector is not known from it.
This is the safer reading — a value that exists is saved rather than lost —
and the camera settles it for this device: `ExposureTime` is saved
(`ExposureTime<TAB>30`) and a load restores it from 20 000 µs to 30 µs.

**Consequence.** A file saved while a selector was unavailable holds the
plain value; loaded when that selector is available again, the value goes to
whatever entry the selector holds then. A selector's availability depending on
the configuration itself (one feature enabling another's selector) is not
modelled beyond the restore order the dependency graph already gives.
