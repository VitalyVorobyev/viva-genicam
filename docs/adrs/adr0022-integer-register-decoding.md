# ADR-0022: Reinterpret Full-Width Unsigned Registers Rather Than Refuse Them

**Status:** Accepted
**Date:** 2026-09-21

## Context

`bytes_to_i64` turns a register payload into the `i64` that GenApi's `IInteger`
is defined over. Two things about a payload decide the answer: its `<Sign>` and
its `<Endianess>`. Until now the decoder honoured the first and discarded the
second, and it refused a class of value outright.

### The refusal

An 8-byte register declared `<Sign>Unsigned</Sign>` can hold values above
`i64::MAX`. The decoder returned

```
node <name> holds an unsigned 64-bit value larger than i64::MAX
```

whenever the top bit was set, which made the node permanently unreadable on any
camera that sets it.

**Two vendors, two reporters.** A Vieworks FS3200T in
[#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112) and a FLIR
Blackfly in [#140](https://github.com/VitalyVorobyev/viva-genicam/issues/140).
These are timestamps, PTP offsets and chunk counters — 64-bit quantities whose
top bit sets in ordinary operation, not exceptionally. #140's reporter counted
12 such registers in their own XML; the corpus holds **196 across 22 of its 38
documents**, and `fixtures/vendor-xml/FLIR_BFS_PGE_31S4C_C.xml` contains the
exact node they quoted.

The refusal was not arbitrary. It came out of ADR-0018, whose table has "always
sign-extend → unsigned unless `<Sign>Signed</Sign>` → every register with the
top bit set". Honouring `<Sign>` was right. This is the one case where
honouring it produces a node nobody can read.

### The specification's position

GenICam v2.1.1 §2.4 states that a GenApi integer *is* an `int64`. A register
holding a value above `i64::MAX` is therefore outside the model the standard
defines, and a strict reading says such a declaration is malformed.

That reading is not available to us. The devices exist, several vendors ship
them, and CLAUDE.md's evidence hierarchy puts real hardware above the
specification for exactly this reason: **the goal is to work with the hardware
that exists, not with the hardware the standard describes.**

### The discarded byte order

Separately and more widely: `NodeDecl::Integer` had no field for byte order.
The parser routed `<Endianess>` / `<Endianness>` / `<ByteOrder>` only into the
bitfield builder, and `BitfieldBuilder::finish` returns `None` unless an `LSB`,
`MSB`, `Bit` or `Mask` set a bit range. So on an unmasked register the declared
order had nowhere to go, and every such register decoded big-endian.

Measured over the corpus's plain `<IntReg>` declarations: **7 167 big-endian in
36 documents, 311 little-endian in 16, 2 undeclared in 1.** The 311 are the
defect, and they are concentrated in the hardware people report on — Point Grey
(89 in one document), FLIR (86 across four), Basler, Hikrobot, Micro-Epsilon.
`i64_to_bytes` carried the same defect, which matters more than it looks: had
only the reader learned byte order, its round-trip validation would have started
rejecting every little-endian write.

Unlike the refusal, this one is plain non-conformance with no tension in it.

## Decision

1. **A full-width unsigned value is reinterpreted as two's complement, not
   refused.** The cast is lossless at the bit level and round-trips through
   `i64_to_bytes`. A caller that wants the unsigned reading casts back with
   `as u64`. No `i128` widening, and no second accessor: the standard says the
   model is `int64`, and widening it would be a larger deviation than the one
   this ADR accepts.

2. **`NodeDecl::Integer` carries `byte_order`,** mirroring `NodeDecl::Float`,
   defaulting to `ByteOrder::Big` per GenICam. The parser records it on the
   declaration *in addition to* the bitfield, so masked registers keep the
   bitfield path they already had.

3. **Both directions of the codec take the byte order.** They are changed in
   one commit and validated against each other.

4. **The reinterpretation is logged** at `debug` once per read, because the
   value the caller sees is negative while the register is not.

## Consequences

### Positive

- The nodes in #112 and #140 become readable. 196 registers across 22 documents
  stop failing.
- 311 little-endian registers across 16 documents stop decoding byte-swapped,
  in both directions.
- The corpus test gains a second evaluation pass (see below) that can fail.

### Negative

- **A `u64` above `i64::MAX` now reads as a negative number.** A PTP timestamp
  in Viva Studio will render as a large negative value until whoever formats
  timestamps decides how to present it. That is a presentation question; it is
  not a reason to widen the model.
- **Reads and writes both change, and that asymmetry is the hazard.** An
  application that worked around the old behaviour on one side only — decoding
  the raw bytes itself while still writing through us, say — will now be
  self-inconsistent. One that worked around both sides stays consistent.
- Adding a field to `NodeDecl::Integer` is breaking: `#[non_exhaustive]` sits on
  the `NodeDecl` enum, not on the variant, so struct-variant literals must be
  updated. This lands in a minor release.

### Relationship to ADR-0018

ADR-0018 says implement the specification rather than a convenient
approximation. This ADR is not a reversal of it. Point 2 above *is* ADR-0018 —
`<Endianess>` is normative and we were ignoring it. Point 1 deviates, knowingly
and narrowly: the specification says these registers cannot exist, hardware says
they do, and the evidence hierarchy resolves that in the hardware's favour. The
deviation is recorded here rather than left implicit, which is what CLAUDE.md
asks for.

### Why nothing caught either defect

`crates/viva-genapi/tests/vendor_corpus.rs` evaluated every node against
`NullIo`, which answers reads with zeros. A byte swap of zero is still zero, and
an unsigned value's top bit is never set — so the weekly `Vendor XML Corpus`
workflow passed on 2026-08-31, 09-07 and 09-14 across a union of 448 plain
`<IntReg>` nodes about which it could say nothing. That is backlog GA-11.

This ADR ships its first slice: the corpus test now evaluates each document
twice, once against `NullIo` and once against a test-local stub returning a
descending byte pattern, which sets the top bit in either byte order. In the
second pass a `GenApiError::Parse` counts as an engine defect, because every
value the pattern produces is representable. Before the codec change that pass
failed on **373 nodes across 19 of the 38 documents**; after it, it is clean.

What the second pass still cannot catch is a value that is merely *wrong* rather
than refused — the corpus test asserts no values, so a byte-order regression
would pass it. Per-document value expectations are the rest of GA-11.
