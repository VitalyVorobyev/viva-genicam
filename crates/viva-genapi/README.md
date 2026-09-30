# viva-genapi

GenApi node map evaluation engine with typed feature access backed by register I/O.

Turns a parsed `XmlModel` into a live `NodeMap` that can read and write camera features through any transport implementing the `RegisterIo` trait.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **NodeMap** -- in-memory node store built from `XmlModel`, with dependency tracking and cache invalidation
- **Typed accessors** -- `get_integer()`, `get_float()`, `get_enum()`, `get_bool()`, `get_string()`, `exec_command()`, and their setters
- **SwissKnife** -- full expression evaluator (arithmetic, comparisons, ternary, logical, bitwise, math functions)
- **pValue delegation** -- Integer, Float, Enum, Boolean, and Command nodes delegate to backing registers
- **Converter / IntConverter** -- formula-based conversions in both directions, so converted features are writable
- **Selector support** -- address switching for features like `GainSelector`
- **Access predicates** -- `pIsImplemented`, `pIsAvailable` and `pIsLocked` evaluated at runtime; a locked feature is refused before the write is sent
- **Skipped nodes** -- a construct the engine cannot build lands in `NodeMap::skipped()` instead of failing the document
- **NullIo** -- offline XML browsing without a camera
- **WASM compatible** -- compiles for `wasm32-unknown-unknown`

## Usage

```bash
cargo add viva-genapi
```

```rust
use viva_genapi::{NodeMap, NullIo};
use viva_genapi_xml::parse;

let xml = std::fs::read_to_string("device.xml")?;
let nodemap = NodeMap::try_from_xml(parse(&xml)?)?;

// Browse the feature tree offline, with no camera attached.
for name in nodemap.node_names() {
    let kind = nodemap.node(name).map(|n| n.kind_name()).unwrap_or("?");
    println!("{name}: {kind}");
}

// NullIo answers every read with zeroes: values that depend only on the XML
// come out right, register-backed ones read as zero.
let pixel_formats = nodemap.enum_entries("PixelFormat")?;
let width = nodemap.get_integer("Width", &NullIo)?;
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-genapi)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
