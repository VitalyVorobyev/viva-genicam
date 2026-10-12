# ADR-0030: Errors carry their causes; the public surface is chosen

**Status:** Accepted
**Date:** 2026-10-12

## Context

The GenTL producer (backlog GT-01, milestone T) will wrap viva-genicam behind
a C ABI that maps every failure to a GenTL error code and every handle to a
Rust type. Whatever is public when it starts is what it freezes. Before this
decision, a good deal was public by accident:

- **Errors flattened their causes into text.** `GenicamError::Transport` and
  `Parse`, `GenApiError::Io` and `Parse`, `XmlError::Xml`, `Invalid` and
  `Transport`, `GigeError::Protocol`, `U3vError::Usb` and `Protocol` all held a
  `String` built with `format!` from the error underneath. A GVCP
  `ACCESS_DENIED` reached a caller as `transport: device reported status
  ACCESS_DENIED (0x8006)`, text with nothing behind it to match on. One
  variant meant two things: `GenApiError::Parse` was raised both for a
  register whose bytes did not convert and for a declaration that made no
  sense, so a failed device read read as bad XML (backlog GA-30), and
  `NodeNotFound` was raised for nodes that exist but declare no register —
  which hid that 143 nested `<IntSwissKnife>` address terms in nine corpus
  documents are dropped (GA-17). `GenApiError::Access` could not say which
  way an access was refused.
- **Every module of `viva-gige`, `viva-u3v`, `viva-service`, `viva-camctl` and
  `viva-fake-gige` was `pub mod`**, so internals were API: GVSP parsing
  helpers, socket plumbing, invented timestamp registers no caller used. The
  facade's `stream`, `frame`, `events`, `chunks`, `evs` and `time` modules
  gave a second path to what its root re-exports.
- **`viva-genapi` exposed `viva_genapi_xml::Addressing`** through public node
  fields, and the node caches with the generation counter they key on.
- **`#[non_exhaustive]` was applied case by case.** Some error enums had it,
  `GigeError` and `TimeError` did not; some records had it, others not.

## Decision

### 1. An error carries its cause, typed

- A failure that comes from another crate, or from `std`, is the variant's
  [`source`](https://doc.rust-lang.org/std/error/trait.Error.html#method.source):
  `#[source]` or `#[from]`, never `format!` into a `String`.
- Where the cause's type is not ours to expose — a transport behind
  `RegisterIo`, the XML reader, a number parser, a ZIP reader, the USB library
  — it is carried as `Box<dyn Error + Send + Sync>`, so the caller can still
  downcast it (`GenApiError::Io` holds the `GigeError`, `U3vError` or the
  facade's `WriteRefused` behind a register access).
- A refusal of our own has a variant with typed fields that name what was
  refused — the shape of `MaskedWriteUnreadable { name, address }` — and any
  fixed description is a `&'static str`. Names and values are data; a
  sentence built at run time is not.
- The facade's transport variants say what it was doing:
  `GenicamError::Gige { during: "claim control", source }`.
- **`Display` includes the cause's message**, as `"{during}: {source}"`. This
  departs from the convention that an error prints its cause or returns it
  from `source`, never both: Python exceptions, Studio's error panel, the
  services' replies and every `error = %e` log line print `{e}` and nothing
  else, and they are where users read errors. A reporter that walks the chain
  sees the cause twice; we accept that.

Per transport, a status is decoded with that transport's table
([ADR-0020](adr0020-per-transport-status-codes.md)): `GigeError::Status`
holds a `GigeStatus`, `U3vError::Status` a `U3vStatus`.

### 2. `#[non_exhaustive]`

1. **Every public error type is `#[non_exhaustive]`**, enums and error
   structs alike: a new failure must not break a caller's `match`.
2. **Every other public enum is `#[non_exhaustive]`** — status codes, pixel
   formats, payload types, node declarations, control states, report targets —
   with two exceptions:
   - **A set closed by definition stays exhaustive**: `AccessDirection` (read,
     write), `ByteOrder`, `Sign`, `StreamDest` (unicast, multicast), `Mtu`
     (measured, assumed), `OpCode` (GVCP's four memory operations), a
     SwissKnife's integer or float output.
   - **A set a sibling crate must implement in full stays exhaustive**, so a
     new variant fails to compile where it has to be implemented instead of
     falling into a wildcard arm at run time: the XML layer's `AccessMode`,
     `CachingMode`, `Addressing`, `AddressTerm`, `IndexOffset`,
     `FloatEncoding` and `Slope`, which `viva-genapi` evaluates, and
     `AcquisitionCommand`, which the services execute. The crates release
     together, so the break never reaches a caller without the
     implementation; a caller outside that matches on one takes it.
3. **A record the library assembles and hands out, and expects to extend**, is
   `#[non_exhaustive]`, with a constructor where callers need to build one:
   `Frame` (`Frame::new`), the XML layer's model and declarations (`XmlModel`,
   `NodeMeta`, `RegisterDecl` …, `SkippedNode::new`, `DocumentIdentity::new`),
   the persistence reports (`LoadReport`, `LoadFailure`, `PlannedFeature`),
   `AckSummary`, and USB3 Vision's `StreamLayout` and `RawFrame`.
4. **A struct a caller fills in, and one that mirrors a fixed layout, stays
   exhaustive.** Callers build option sets and identities by struct literal
   (`ActionParams`, `McOptions`, `ChunkConfig`, `ReportRequest`, `Roi`,
   `ValueRange`, `DeviceInfo { model, ..DeviceInfo::from_ip(ip) }`,
   `U3vDeviceInfo` for a fake, an `Event` replayed to read its features); a
   decoded header or register block (`GvspHeader`, the leaders and trailers,
   `Abrm`/`Sbrm`/`Sirm`, `FrameHeader`, `BitField`) and the feature file's
   records (`Configuration`, `SavedValue`, `Omission`) have the fields their
   layout has. Adding a field to one is a breaking change, and we take it when
   it comes rather than make every caller go through setters for it.

### 3. The public surface is chosen

- **No blanket `pub mod`.** A crate's modules are private, and its root
  re-exports what callers use. A module stays public only as a namespace of
  vocabulary: `viva_gige::consts` (GVCP opcodes, flags and bootstrap register
  addresses), `viva_u3v::registers` (ABRM, SBRM, SIRM and capability offsets),
  `viva_genapi::swissknife` and `persistence`, and the facade's `blocking`,
  `report` and `image_io`, whose functions (`connect`, `build_report`,
  `encode`) need the module's name, besides its re-exports of the crates below
  it (`viva_genicam::gige`, `u3v`, ...). `viva-service` exports what its
  binary and the USB3 Vision service need, `viva-camctl` only `run`.
- What no caller uses is made private, or deleted when nothing uses it at all.
- **`viva-genapi` never exposes `viva_genapi_xml::Addressing`**
  (`AddressTerm`, `IndexOffset`): it keeps changing (ADR-0026 added the port
  and signed addresses; GA-09 will add `<pLength>`). Callers reach a
  register by name, through `NodeMap::register_address`, and a node's size
  and port through `declared_len()` and `port()`. Every other XML type a
  public node field holds is re-exported, so the crate's API can be named
  from the crate.
- A node's caches are the node map's (`pub(crate)`), for every node kind.
- The library crates set the `missing_docs` and `unreachable_pub` lints
  (workspace `[workspace.lints]`), and what they find is fixed, not allowed.

### 4. Vocabulary is shared; behaviour is not

The fake cameras import the transport crates' register addresses, opcodes and
status codes instead of keeping copies. A constant is vocabulary: the fake
still encodes and decodes every packet and register from the specification,
so a fake that imports an address can still disagree with the client about
what lives there — which is what
[ADR-0019](adr0019-transport-conformance-and-spec-derived-fakes.md) needs.
The golden byte fixtures stay literal.

## Consequences

### Positive

- The GenTL producer maps errors by variant and source, not by parsing text:
  `GigeStatus::Common(AccessDenied)` is `GC_ERR_ACCESS_DENIED` wherever it
  comes from.
- Python, Studio and the services print what they printed before, cause
  included, and code can now branch on the cause.
- The corpus test tells a register that does not convert from a declaration
  that does not evaluate, and counts each (GA-30).
- A type is public because a caller needs it. The public-API listing
  (`cargo public-api`) is the record of what that is.

### Negative

- Every error variant changed shape: a breaking change for every caller that
  matched on `Transport`, `Parse`, `Io`, `Access`, `Protocol` or `Usb`. The
  release notes list them.
- `Box<dyn Error>` sources need a downcast to inspect; the type a transport
  uses is documented, not checked by the compiler.
- Printing a chain duplicates the cause's message (Decision 1).
- `DeviceInfo` and the other caller-built structs stay exhaustive, so a field
  added to one breaks literal construction.
