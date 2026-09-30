# Contributing

Contributions are welcome! Please open an issue or pull request on
[GitHub](https://github.com/VitalyVorobyev/viva-genicam).

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

[Running the tests](testing.md) lists the individual test suites, and
[Testing without hardware](tutorials/fake-camera.md) explains the fake cameras
they run against.

The book's Rust snippets are included from the examples in
`crates/viva-genicam/examples/` through `// ANCHOR:` markers, so they compile
with the rest of the workspace. When you add a snippet to the book, anchor it
in an example rather than writing it inline.

## Code Style

- Follow `rustfmt` defaults
- Keep `clippy` warnings clean
- Add doc comments to all public items
