# GenApi XML

Goal of this tutorial:

- Understand **what** the GenICam XML is and **where** it lives.
- See how `viva-genapi-xml` fetches it from the device and parses it.
- Fetch it yourself, from the CLI and from Rust.
- Know when you actually need to look at it.

You should already have worked through [Discovery](./discovery.md) and
[Registers & features](./registers.md).

---

## 1. What is the GenICam XML?

Every GenICam-compliant device carries a **self-description document**:

- It lists every **feature** the device supports — name, type, access mode,
  range.
- It defines how those features map to **device registers**.
- It encodes **categories**, **selectors**, and **SwissKnife** expressions.
- It declares which **GenApi schema version** the document uses.

The document normally lives in the device's non-volatile memory. To get it, the
host:

1. Reads the **`GevFirstURL`** register at `0x0200`.
2. Interprets the result as a URL saying where the document actually is —
   usually `local:` plus a memory address and length, in principle also
   `http://` or `file://`.
3. Reads those bytes.
4. Hands the string to a GenApi implementation.

Two details that the specification permits and real devices use:

- If `GevFirstURL` is empty or its document cannot be retrieved,
  **`GevSecondURL` at `0x0400`** is tried next.
- The document is often **ZIP-compressed**. `viva-genapi-xml` decompresses it
  transparently, subject to a 64 MiB cap so a malformed length field cannot
  exhaust memory.

---

## 2. The shape of the API

`viva-genapi-xml` exposes three things you are likely to call:

```rust,ignore
// Follow the URL registers and return the document.
pub async fn fetch_and_load_xml<F, Fut>(read_mem: F) -> Result<String, XmlError>
where
    F: FnMut(u64, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, XmlError>>;

// Cheap, deliberately lossy: schema version and top-level names.
pub fn parse_into_minimal_nodes(xml: &str) -> Result<MinimalXmlInfo, XmlError>;

// The full parse: every node declaration, plus the ones that had to be skipped.
pub fn parse(xml: &str) -> Result<XmlModel, XmlError>;
```

Application code rarely calls these directly — `connect_gige` does it for you.
They matter when you are debugging why a feature behaves as it does, inspecting
how a vendor encoded something, or adding support for a construct
`viva-genapi` does not handle yet.

---

## 3. Getting the XML

### 3.1. From the command line

```bash
cargo run -p viva-camctl -- xml --ip 192.168.0.10 --out camera.xml
```

This never parses the document, so it works on a camera the library
cannot open — which is the only camera anyone ever needs it for. If you are
reporting a problem, send this: see
[Reporting a camera we can't open](../reporting.md).

### 3.2. From Rust

`fetch_and_load_xml` knows nothing about GVCP, sockets or cameras. It calls a
closure with `(address, length)` and expects bytes back, so any transport that
can read device memory can drive it:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/fetch_xml.rs:fetch}}
```

Run it with:

```bash
cargo run -p viva-genicam --example fetch_xml
```

---

## 4. Inspecting the document

`parse_into_minimal_nodes` answers the cheap questions — which schema version,
what is at the top level, does this look broken at all:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/fetch_xml.rs:inspect}}
```

It is intentionally lossy. It does not understand every node type; its job is to
be fast and to survive schema extensions that are not implemented yet.

`parse` is the full path, and it is what `NodeMap` is built from. It returns an
`XmlModel` with a flat list of node declarations carrying:

- Feature name and type (Integer, Float, Enumeration, Boolean, Command,
  Category, SwissKnife, Converter, …).
- Addressing: fixed, selector-based, or indirect through `pAddress`.
- Access mode, bitfield layout and byte order.
- Selector relationships and expression text.

### Skipped nodes

A construct the parser cannot handle does not fail the whole document — it
goes into `XmlModel::skipped`, and the corresponding GenApi-level list is
`NodeMap::skipped()`. Both are logged.

Degrading to "this one feature is missing" is far better than "this camera does
not work", and the vendor-corpus tests fail on any skip that is not on their
allowlist, so new gaps surface rather than accumulate.

---

## 5. From XML to a NodeMap

`viva-genapi` takes the `XmlModel` and:

- Instantiates a `NodeMap`.
- Resolves feature dependencies, `pValue` delegation, selectors and expressions
  at access time rather than at load time.
- Invalidates cached values when something they depend on changes.

You do not do this plumbing yourself in an application: `connect_gige` fetches,
parses and builds, and `viva-camctl get` / `set` use the same pipeline. See the
[viva-genapi chapter](../crates/viva-genapi.md) for the internals.

---

## 6. When should you look at the XML?

Most of the time, treat it as an implementation detail. Crack it open when:

- A feature behaves differently from what SFNC describes.
- Selectors are not doing what you expect.
- You hit a SwissKnife or bitfield corner case.
- You are adding support for a vendor-specific wrinkle.

A workable order: dump it with `viva-camctl xml`, run `fetch_xml` for the schema
version and skip list, then read the document itself in an XML viewer for the
category you care about.

If you have a camera the library cannot open, that document is the single most
useful thing you can send us — see
[Reporting a camera we can't open](../reporting.md).

---

## 7. Recap

You should now:

- Know what the GenICam XML is, where it lives, and how the URL registers point
  at it.
- Be able to fetch it with `viva-camctl xml` or `fetch_and_load_xml`.
- Know the difference between the minimal scan and the full parse, and what a
  skipped node means.

Next: [Streaming](./streaming.md) — getting image data out, now that you know
how the camera describes itself.
