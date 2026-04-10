# Contributing

Contributions are welcome! Please open an issue or pull request on
[GitHub](https://github.com/VitalyVorobyev/genicam-rs).

## Development Setup

```bash
# Build the workspace
cargo build --workspace

# Run tests (includes fake camera integration tests)
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Code Style

- Follow `rustfmt` defaults
- Keep `clippy` warnings clean
- Add doc comments to all public items
