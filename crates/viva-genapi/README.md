# viva-genapi

GenApi node map evaluation engine with typed feature access backed by register I/O.

Turns a parsed `XmlModel` into a live `NodeMap` that can read and write camera features through any transport implementing the `RegisterIo` trait.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **NodeMap** -- in-memory node store built from `XmlModel`, with dependency tracking and cache invalidation
- **Typed accessors** -- `get_integer()`, `get_float()`, `get_enum()`, `get_bool()`, `exec_command()`, etc.
- **SwissKnife** -- full expression evaluator (arithmetic, comparisons, ternary, logical, bitwise, math functions)
- **pValue delegation** -- Integer, Float, Enum, Boolean, and Command nodes delegate to backing registers
- **Converter / IntConverter** -- linear and polynomial value conversions
- **Selector support** -- address switching for features like `GainSelector`
- **NullIo** -- offline XML browsing without a camera
- **WASM compatible** -- compiles for `wasm32-unknown-unknown`

## Usage

```toml
[dependencies]
viva-genapi = "0.1"
```

```rust
use viva_genapi::{NodeMap, RegisterIo, NullIo};
use viva_genapi_xml::parse;

let xml = std::fs::read_to_string("device.xml")?;
let model = parse(&xml)?;
let nodemap = NodeMap::from(model);

// Browse features offline
let io = NullIo;
for name in nodemap.node_names() {
    println!("{name}");
}
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-genapi)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
