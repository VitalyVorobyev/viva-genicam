# viva-pfnc

Pixel Format Naming Convention (PFNC) tables and helpers for GenICam.

Maps numeric pixel format codes to human-readable names, bit depths, and layout metadata.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **PixelFormat enum** -- the monochrome, Bayer (8- and 16-bit), packed RGB/BGR, confidence and 3D-coordinate formats, two vendor-defined event-vision formats, and `Unknown(u32)` for any other code. The enum is `#[non_exhaustive]`; see the API reference for the current list
- **Code and name conversion** -- `from_code(u32)`, `code() -> u32` and `from_name(&str)`
- **Layout helpers** -- `bytes_per_pixel()`, `is_bayer()`, `cfa_pattern()`
- **Optional serde** -- enable the `serde` feature for serialization support

## Usage

```bash
cargo add viva-pfnc
```

```rust
use viva_pfnc::PixelFormat;

let fmt = PixelFormat::from_code(0x0108_0001);
assert_eq!(fmt, PixelFormat::Mono8);
assert_eq!(fmt.bytes_per_pixel(), Some(1));
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `serde` | No | Derive `Serialize`/`Deserialize` for `PixelFormat` |

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-pfnc)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
