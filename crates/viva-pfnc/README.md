# viva-pfnc

Pixel Format Naming Convention (PFNC) tables and helpers for GenICam.

Maps numeric pixel format codes to human-readable names, bit depths, and layout metadata.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **PixelFormat enum** -- Mono8, Mono16, BayerRG8, BayerGB8, BayerBG8, BayerGR8, RGB8Packed, BGR8Packed
- **Code conversion** -- `from_code(u32)` and `code() -> u32` for PFNC numeric values
- **Layout helpers** -- `bytes_per_pixel()`, `is_bayer()`, `cfa_pattern()`
- **Optional serde** -- enable the `serde` feature for serialization support

## Usage

```toml
[dependencies]
viva-pfnc = "0.1"
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
