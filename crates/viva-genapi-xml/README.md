# viva-genapi-xml

GenICam XML parser: loads device description files into a strongly-typed intermediate representation.

Parses GenICam XML node maps into `XmlModel` with typed declarations for all standard node types (Integer, Float, Enumeration, Boolean, Command, Category, SwissKnife, Converter, IntConverter, String, Register). A node the parser cannot handle is recorded in `XmlModel::skipped` instead of failing the whole document.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Full XML parsing** -- parse GenICam XML into `XmlModel` with typed `NodeDecl` variants
- **Minimal parsing** -- `parse_into_minimal_nodes()` for a cheap scan of the schema version and top-level names
- **XML fetch** -- download and decompress GenICam XML from a device (behind the `fetch` feature flag)
- **Serde support** -- all public types derive `Serialize`/`Deserialize`
- **WASM compatible** -- compiles for `wasm32-unknown-unknown`

## Usage

```bash
cargo add viva-genapi-xml
```

```rust
use viva_genapi_xml::{parse, XmlModel};

let xml = std::fs::read_to_string("device.xml")?;
let model: XmlModel = parse(&xml)?;
println!("{} nodes parsed", model.nodes.len());
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `fetch` | Yes | Enable `fetch_and_load_xml()` for downloading XML from devices |

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-genapi-xml)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
