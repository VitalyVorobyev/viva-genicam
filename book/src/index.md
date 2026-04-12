# viva-genicam

Welcome to the viva-genicam workspace. This project provides Rust building blocks
for GenICam-compatible transports, control, and feature access with a focus on
GigE Vision.

## Quickstart

- Install Rust via `rustup` (toolchain pinned in `rust-toolchain.toml`).
- Clone the repository and run `cargo test --workspace`.
- Explore the facade crate with `cargo run -p viva-genicam --example list_cameras`.

## Crates

- `viva-gencp`: GenCP encode/decode primitives.
- `viva-genapi-xml`: XML fetch and minimal parsing helpers.
- `viva-genapi`: NodeMap evaluation and feature access.
- `viva-gige`: GigE Vision transport utilities.
- `viva-genicam`: Facade re-export combining the workspace crates.

See the main [README](https://github.com/VitalyVorobyev/viva-genicam/blob/main/README.md)
for status updates and roadmap details.
