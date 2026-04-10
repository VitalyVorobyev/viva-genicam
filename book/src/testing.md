# Testing

## Unit Tests

```bash
cargo test --workspace
```

Unit tests are embedded in source modules (`mod tests { }`).

## Integration Tests (Fake Camera)

The workspace includes `viva-fake-gige`, an in-process GigE Vision camera simulator.
Integration tests run automatically with `cargo test` -- no external tools needed.

```bash
# Run integration tests specifically
cargo test -p viva-genicam --test fake_camera
```

The fake camera provides:
- GVCP discovery on UDP (loopback)
- GenCP register read/write with an embedded GenApi XML
- GVSP streaming with synthetic image frames

## Integration Tests (Aravis)

For conformance testing against a third-party implementation, the aravis fake
camera can be used:

```bash
brew install aravis  # macOS
cargo test -p viva-genicam --test fake_camera -- --ignored --test-threads=1
```
