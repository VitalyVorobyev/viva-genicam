# Error Handling & Logging

## Error Types

Each crate defines its own error type:

- `GenicamError` -- high-level facade errors
- `GigeError` -- GVCP/GVSP transport errors
- `GenApiError` -- node evaluation and register I/O errors
- `GenCpError` -- GenCP protocol encoding errors
- `XmlError` -- XML parsing errors

All error types implement `std::error::Error` and `Display`.

## Logging

The workspace uses the [`tracing`](https://docs.rs/tracing) crate for structured logging.
Enable it with:

```rust
tracing_subscriber::fmt::init();
```

Or set `RUST_LOG=debug` to see detailed protocol traces.
