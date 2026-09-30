# `viva-genapi` — the NodeMap

`viva-genapi` turns the `XmlModel` produced by `viva-genapi-xml` into a
**`NodeMap`**: the thing that knows what `ExposureTime` means, where it lives,
what it depends on, and whether you are allowed to write it right now.

It has no transport of its own. Every accessor takes a `&dyn RegisterIo`, which
is the whole coupling between this crate and the wire:

```rust,ignore
pub trait RegisterIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError>;
    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError>;
}
```

Two implementations ship with the stack: `GigeRegisterIo` and `U3vRegisterIo`
in `viva-genicam`, and **`NullIo`** here — which returns zeroes for reads and
discards writes. `NullIo` is not a placeholder: it is how a GenApi document is
browsed with no camera attached, which is what Viva Studio and the WASM build
do. Values that depend only on the XML — SwissKnife expressions over constants,
enum entry lists, ranges, the category tree — come out correct; anything backed
by a real register comes out zero, so read structure from it, not data.

---

## Node kinds

| Variant | GenICam name |
|---|---|
| `Node::Integer` | `Integer`, `IntReg`, `MaskedIntReg` |
| `Node::Float` | `Float`, `FloatReg` |
| `Node::Enum` | `Enumeration` |
| `Node::Boolean` | `Boolean` |
| `Node::Command` | `Command` |
| `Node::Category` | `Category` |
| `Node::SwissKnife` | `SwissKnife`, `IntSwissKnife` |
| `Node::Converter` | `Converter` |
| `Node::IntConverter` | `IntConverter` |
| `Node::String` | `StringReg` |
| `Node::Register` | `Register` (plain `<Length>` only; `<pLength>` is not supported yet) |

`Node::kind_name()` returns the GenICam name, `name()` the feature name, and
`access_mode()` the declared mode — which is not the same as the effective one,
see below.

---

## Reading and writing

The accessors are typed, and each takes the transport:

```rust,ignore
let width  = nodemap.get_integer("Width", &io)?;
let expo   = nodemap.get_float("ExposureTime", &io)?;
let fmt    = nodemap.get_enum("PixelFormat", &io)?;
let on     = nodemap.get_bool("ReverseX", &io)?;
let serial = nodemap.get_string("DeviceSerialNumber", &io)?;

nodemap.set_integer("Width", 640, &io)?;
nodemap.exec_command("AcquisitionStart", &io)?;
```

`Camera` in `viva-genicam` wraps these behind string-valued `get`/`set`, which
is what most application code should use — see
[Registers & features](../tutorials/registers.md).

### Addressing

A node's address is not always a constant. It can be:

- **Fixed** — an `<Address>` in the XML.
- **Computed** — a sum of `<Address>`, `<pAddress>` (another node's value) and
  `<Integer>` offsets, resolved at access time.
- **Delegated** — `<pValue>` pointing at another node, which may itself
  delegate. The declaration you are reading is often not the one holding the
  data.

This matters because it is where the interesting bugs live. A parser can accept
a document perfectly and the address model still be wrong, which is why the
[vendor corpus test](../reporting.md#what-happens-to-it) does not stop at
parsing: it builds a `NodeMap` from each real document and evaluates every
node.

### Selectors

When a feature is selector-dependent (`Gain` behind `GainSelector`), the
selector's current value takes part in resolving the address. Writing the
selector invalidates the cached values of everything that depends on it, so the
next read of `Gain` returns the newly selected channel rather than a stale
value.

### SwissKnife and Converter

`SwissKnife` nodes compute a value from other nodes with a formula — arithmetic,
comparisons, bitwise operators, and a ternary. `Converter` and `IntConverter`
run a formula in both directions, so they are writable: the `FROM` expression
maps a user value back to the raw one. Evaluation is transparent — you call
`get_float` and the inputs are read or computed first.

---

## Access mode is evaluated, not declared

`AccessMode` in the XML is a starting point. The effective mode also depends on
predicates the device answers at runtime:

- `pIsImplemented` — the feature is absent on this model.
- `pIsAvailable` — present but not currently applicable.
- `pIsLocked` — writable in principle, locked right now.

`effective_access_mode`, `is_implemented`, `is_available` and
`available_enum_entries` expose this. Writes go through it: a locked node is
refused **locally**, with `GenApiError::Locked { name, locked_by }` naming the
feature that holds the lock — because "access denied" leaves a caller nowhere
to go, whereas "`ExposureTime` is locked by `ExposureTime_Lck`" says what to
change first.

---

## Introspection

Built for consumers that need to render a feature tree rather than read one
value:

| Method | Returns |
|---|---|
| `node_names()` | Every feature name |
| `node(name)` | The `Node`, or `None` |
| `categories()` | Category name → its members |
| `dependents(name)` | Which nodes are invalidated when this one changes |
| `nodes_at_visibility(level)` | `Beginner` / `Expert` / `Guru` / `Invisible` filtering |
| `version()` | The document's schema version |
| `skipped()` | Nodes that could not be built |

`skipped()` is the important one. A construct this crate cannot handle does
not fail the whole document — it lands here, and the parser's own losses
(`XmlModel::skipped`) are carried along with it, so a consumer sees both. The
camera still opens, minus the features that could not be built.

---

## Caching and invalidation

Values are cached and invalidated by dependency: writing a node clears the
cached values of everything `dependents()` lists for it, including through
`pValue` delegation and selector relationships. Cache correctness is a
conformance question, not an optimisation — a stale `Width` after a selector
change is a wrong answer, not a slow one.

---

## Errors

`GenApiError` is specific on purpose; the variant usually says what to do next:

| Variant | Means |
|---|---|
| `NodeNotFound(name)` | No such feature in this camera's document |
| `Type(name)` | Asked for the wrong type — `get_float` on an `Integer` |
| `Access(name)` | The node's access mode forbids the operation |
| `Locked { name, locked_by }` | `pIsLocked` is engaged; `locked_by` is the feature to change |
| `Range(name)` | Value outside `Min`/`Max`, or off `Inc` |
| `Unavailable(name)` | Hidden by the current selector state |
| `Io(msg)` | The transport failed |
| `Parse(msg)` | Metadata or conversion failure |
| `ExprParse` / `ExprEval` / `UnknownVariable` | A SwissKnife formula failed to parse, evaluate, or resolve a reference |
| `EnumValueUnknown` / `EnumNoSuchEntry` | Raw value maps to no entry, or no entry has that name |
| `BadIndirectAddress` | `pAddress` resolved to something impossible |
| `BitfieldOutOfRange` / `ValueTooWide` | Bitfield metadata exceeds the register, or the value exceeds the field |

---

## For contributors

- Keep evaluation pure and reach the device only through `RegisterIo`. That
  separation is what makes `NullIo` and the corpus test possible.
- New node kinds go behind the same evaluation path, and must land in
  `skipped()` rather than aborting the document when something is unsupported.
- Test against the specification, not against our own parser. When the two
  disagree, the specification wins unless real hardware says otherwise —
  [ADR-0018](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0018-genapi-conformance-over-convenience.md)
  lists eight defects that each looked reasonable in isolation.

---

## See also

- [GenApi XML tutorial](../tutorials/genapi-xml.md) — where the document comes from
- [Registers & features](../tutorials/registers.md) — the same thing from an application's side
- [`viva-gige`](viva-gige.md) — the transport that backs `GigeRegisterIo`
