# viva-pfnc

Pixel Format Naming Convention (PFNC) tables and helpers for GenICam.

Maps numeric pixel format codes to human-readable names, bit depths, and layout metadata.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **PixelFormat enum** -- monochrome and Bayer at 8 to 16 bits, including the packed `Mono10p`/`Mono12p` and GigE Vision `Mono10Packed`/`Mono12Packed` families; RGB/BGR with or without alpha; 8-bit YUV/Y'CbCr at 4:1:1, 4:2:2 and 4:4:4; confidence and 3D-coordinate formats; two vendor-defined event-vision formats; and `Unknown(u32)` for any other code. Codes follow EMVA's *GenICam Pixel Format Names and Values*. The enum is `#[non_exhaustive]`; see the API reference for the current list
- **Code and name conversion** -- `from_code(u32)`, `code() -> u32` and `from_name(&str)`, which also accepts the older names cameras report (`YUV422Packed`, `RGB8Packed`)
- **Sizes** -- `bits_per_pixel()`, `bytes_per_pixel()`, and `line_bytes()`/`image_bytes()`, exact for packed formats
- **Layout helpers** -- `is_mono()`, `is_bayer()`, `cfa_pattern()`
- **Conversion** (`viva_pfnc::convert`) -- unpack to `u16` samples, reduce to 8 bits, bilinear Bayer demosaic, Y'CbCr to RGB; `Converter` reuses its buffers between frames. No I/O and no dependencies; compiles for `wasm32-unknown-unknown`
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

// Two 12-bit pixels in three bytes, GigE Vision layout.
let rgb = viva_pfnc::convert::to_rgb8(PixelFormat::Mono12Packed, 2, 1, &[0xAB, 0x3C, 0x12]).unwrap();
assert_eq!(rgb, [0xAB, 0xAB, 0xAB, 0x12, 0x12, 0x12]);
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `serde` | No | Derive `Serialize`/`Deserialize` for `PixelFormat` |

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-pfnc)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
