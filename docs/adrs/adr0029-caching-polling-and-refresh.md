# ADR-0029: Caching modes, polling and a changed-since query

**Status:** Accepted
**Date:** 2026-10-12

## Context

Until 0.9 a node map cached every value it read until a write invalidated
it. That is right for a value only the host changes, and wrong for one the
camera changes on its own: a sensor temperature, an exposure under auto
exposure, a status bit. Read twice, such a value came back the same. The
XML says which values these are, and we parsed none of it (backlog GA-05).
Applications had one tool, `NodeMap::invalidate(name)`, and a viewer had no
way to learn what to read again (GA-40).

**What the standard says** (GenICam Standard v2.1.1):

- §2.8.3, on `<Register>` and every register kind derived from it: "The
  `<Cacheable>` element can have the values NoCache, WriteThrough, and
  WriteAround. WriteThrough means that a value written to the camera is
  written to the cache as well. WriteAround means that only read values are
  written to the cache. The latter behavior makes sense, for example, with an
  IFloat::Gain node where the user can write any value, but when reading
  back, will retrieve a value that has been rounded by the camera". The prose
  spells the element `Cacheable`; its example and the schema write
  `Cachable`, and the example's value is `No`.
- §2.8.3: "The `<PollingTime>` element denotes a recommended time interval
  [in ms] after which a node should be invalidated. Note that polling is an
  optional feature of any implementation and the polling time is a hint
  only." The example is a `SensorTemperature` register, "not cacheable. If
  displayed, it should be polled every 10000 ms."
- §3.2, defaults: `Cachable` is `WriteAround` for a `<FloatReg>` and
  `WriteThrough` for `<IntReg>`, `<MaskedIntReg>`, a `<StructEntry>` and the
  register element template every other register uses.
- §2.8.8, `<Command>`: "IsDone always read the device register regardless of
  the state of the caches"; "A `<PollingTime>` entry can be used to handle
  self-clearing commands: While the command is active the node is invalidated
  each time the PollingTime expires. If a call to IsDone reveals that the
  command is gone idle the polling stops. If the command is write only no
  polling takes place."
- §2.8.10, `<Enumeration>`: "the polling takes only place while the
  enumeration's value is set to the value of an EnumEntry which has a
  `<IsSelfClearing>` entry set to Yes … If the node's value is read while
  Action==Active the reading will be performed ignoring the cache."
- §2.6 and §2.4 tie caching to registers ("depending on the Cacheable
  attribute of the corresponding IntReg nodes") and say "all clients of a node
  are informed if the node changes its content so that the clients can
  invalidate their caches". **Nothing says whether a node in front of a
  register — a `<Float>` whose `<pValue>` is a `NoCache` register, a
  `<Converter>` reading it — inherits the register's mode.** §2.8.1's
  `<pBlockPolling>` stops a node's polling while its target is non-zero.

**The reference implementation's public headers** (GenApi, as vendored by
roboception's `rc_genicam_api`; corroboration, not authority):
`INodeMap::Poll(int64_t ElapsedTime)` "Fires nodes which have a polling
time"; `INode::GetPollingTime` is the "recommended polling time (for not
cachable nodes)"; `INode.h`'s `Combine(Peter, Paul)` returns `NoCache` if
either mode is, else `WriteAround` if either is, else `WriteThrough`. aravis
caches at the register only, with the standard's defaults except a
`<StructReg>` (`NoCache`) and a `<StructEntry>` (`WriteAround`), reads any
value but `WriteAround` and `WriteThrough` as `NoCache`, and parses
`<PollingTime>` without using it.

**The corpus** (38 documents, whole-element match): 3 253 `<Cachable>` in 35
documents — `WriteThrough` 2 026, `NoCache` 883, `WriteAround` 344 — on
`IntReg` 1 937, `MaskedIntReg` 1 134, `FloatReg` 108, `StringReg` 51,
`StructReg` 10, `Register` 7, `StructEntry` 6: never on a node without a
register. 365 `<PollingTime>` in 30 documents: `IntReg` 287, `FloatReg` 55,
`MaskedIntReg` 21, `Command` 2 (Sony's `UserSetLoad`/`UserSetSave`, 5000 ms,
both write-only); 500 ms 159 times, 1000 ms 103. 546 of the `NoCache`
registers declare no polling time. No `<pBlockPolling>`, no enumeration with
a polling time, no `Cacheable` spelling. FLIR's `DeviceTemperature` and
`ExposureTime` are each a `<Float>` over a `<Converter>` over an `<IntReg>`
that is `NoCache` and polled every 500 ms.

## Decision

**Parse what the XML says, where it says it.** `NodeMeta` records
`cachable` and `polling_time_ms` as declared, on every node kind — the
default is resolved in `viva-genapi`, where a node's kind is known. A
`<StructEntry>` without its own takes its `<StructReg>`'s. `Cacheable` is
accepted beside `Cachable`; `No`, and any value the standard does not
define, read as `NoCache`. A `<PollingTime>` that is not a number is dropped
with a warning, not the node. `EnumEntryDecl` records `<IsSelfClearing>`.

**The three modes, as the standard defines them.** A `NoCache` value is
never answered from a cache — nor is the read-modify-write of a bit in a
`NoCache` register, whose other bits are read fresh. After a write to a
`WriteAround` register nothing is cached, so the next read shows what the
camera stored; a read is cached. `WriteThrough` is what every register did
before. Defaults: `WriteAround` for a `<FloatReg>` (a `<Float>` with an
address of its own, which only `<FloatReg>` is in a standard document),
`WriteThrough` for every other register. A write-only register keeps the
bytes it was written whatever its mode: they are the only way to preserve its
other bits (GA-31).

**A node is cached only as far as every node its value is read through
allows.** Where the standard is silent we take the reading that never answers
with a stale value: a node's mode is its own (or its register default)
combined, as `Combine` does, with the mode of every node on its value path —
`<pValue>`, a formula's variables, a converter's `<pValue>` and the variables
of its `<FormulaFrom>`. So FLIR's `DeviceTemperature`, its converter and its
register are all `NoCache`: 862 of the 1 688 `NoCache` values the corpus test
reads twice are `NoCache` only through a register they read. A node whose
value path reads a polled register keeps no cache of its own, so the
register's expiry decides. The other direction is not implemented: a
`<Cachable>` on a node without a register does not reach the registers behind
it. The standard places the element on registers, and so do all 3 253 corpus
declarations.

**`<PollingTime>` expires a cached value, against an injected clock.** A
polled value read from the device is the device's for its polling time; the
next read after that goes to the device. The node map asks a `Clock` for the
time — `SystemClock` (`std::time::Instant`) by default, a `ManualClock` that
tests and hosts without a system clock move by hand, `NodeMap::set_clock` to
choose — because `Instant::now()` panics on `wasm32-unknown-unknown`, where
Studio browses XML offline. There the default is a `ManualClock` that stands
still: polled values never expire until the host injects a clock. The clock
is consulted only for a node with a polling time of its own; a document
without one never asks it. A command is polled from its execution until
`IsDone` finds it finished, unless it is write-only;
`Camera::execute_and_wait` asks `IsDone` at the command's polling time
instead of every `COMMAND_POLL_INTERVAL` when it declares one. An
enumeration with a polling time is polled, and read past every cache, while
its value is a self-clearing entry. `<pBlockPolling>` is not implemented: no
corpus document uses it.

**A changed-since query in place of callbacks.** `NodeMap` is not `Sync` —
its caches are `RefCell`s — and every consumer we have (Studio, the services,
Python) already reads on its own schedule, so a query fits where callbacks
would need a thread-safe registry. Every write and every invalidation
advances a generation (`NodeMap::generation`) and records it against the
nodes it touched: the node written, the registers behind it, everything that
depends on them. `changed_since(generation)` returns those changed after it,
in document order, and the generation now; a read changes nothing.
`poll()`, after `INodeMap::Poll`, invalidates each polled value whose polling
time has passed since it was read from the device, and returns its name with
those of every node read through it — once, until it is read again; a
running command and an active self-clearing enumeration are reported every
period. A viewer keeps the generation it drew, calls `poll`, and re-reads
what `changed_since` names among the features it shows. `invalidate(name)`
and `invalidate_all()` remain for a change the XML does not describe.

**What a viewer needs, on every feature.** `NodeMap::polling_time(name)` is
the shortest polling time on the node's value path and
`NodeMap::caching_mode(name)` its combined mode; `FeatureState` carries both,
and the Zenoh wire state carries them as `polling_ms` and `caching`, so a
remote viewer polls what a local one does. The `Camera` facade exposes
`invalidate`, `invalidate_all`, `generation`, `changed_since` and `poll`,
and so does Python.

**Viva Studio polls the features on screen** at their polling time, with a
floor of 50 ms — the shortest polling time in a vendor document of the
corpus — and only while they are shown, features due together going out as
one bulk read. A `NoCache` feature with no polling time is read every
1000 ms: the camera changes it with nothing to announce it, and where vendors
do say how often, 500 and 1000 ms are the commonest values; the slower one
keeps a screenful of such features to a read each per second. Remote mode
needs no service change: the service answers with the same state.

## Consequences

**Positive**

- A value the camera changes on its own reads fresh, through every node in
  front of it, and a viewer polls it at the camera's own interval.
- A gain or exposure the camera rounds reads back as the camera stored it.
- A consumer learns what changed without callbacks, threads or a `Sync` node
  map, and `wasm32-unknown-unknown` still builds and runs.

**Negative**

- More register reads: every `NoCache` value is read each time it is asked
  for, and a write to a `<FloatReg>` without `<Cachable>` — `WriteAround` —
  is followed by a device read on the next get. That is the cost the XML asks
  for.
- Whether a camera reports a `WriteAround` register's new value at once, or
  the old one until it has applied the write, only hardware can say
  (GA-47). Before 0.9 the written value was shown either way.
- Polling is lazy: a value expires when it is next read, and `changed_since`
  learns about elapsed polling times only from `poll`. A consumer that wants
  them must call it.
- A `<Cachable>` on a node without a register limits that node's own cache,
  not the registers behind it, and `<pBlockPolling>` is not read (GA-48);
  neither occurs in the corpus.
