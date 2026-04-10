# Testing

## Unit Tests

```bash
cargo test --workspace
```

Unit tests are embedded in source modules (`mod tests { }`).

## Integration Tests

The workspace includes `viva-fake-gige`, an in-process GigE Vision camera
simulator. All integration tests run automatically with `cargo test` -- no
external tools or hardware required.

```bash
# Run all tests (unit + integration)
cargo test --workspace

# Run integration tests specifically
cargo test -p viva-genicam --test fake_camera

# Run viva-service end-to-end tests (Zenoh bridge)
cargo test -p viva-service --test fake_camera_e2e
```

The fake camera supports:
- GVCP discovery on UDP (loopback)
- GenCP register read/write with an embedded GenApi XML
- GVSP streaming with synthetic image frames and real timestamps
- Chunk data (timestamp, exposure time) when ChunkModeActive is enabled
- Timestamp features (GevTimestampTickFrequency, GevTimestampValue, TimestampLatch)

## Demo

Run the self-contained demo to see the full workflow without hardware:

```bash
cargo run -p viva-genicam --example demo_fake_camera
```

This starts a fake camera, discovers it, reads/writes features, and streams
frames -- all on localhost with zero setup.

## Manual / Interactive Testing

For interactive testing or E2E testing with genicam-studio, start the fake
camera as a standalone server:

```bash
# Stays alive until Ctrl+C
cargo run -p viva-fake-gige
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15
```

Then use `viva-camctl` or `viva-service` to interact with it. See the
[Testing without hardware](tutorials/fake-camera.md) tutorial for details.
