# viva-gencp

Transport-agnostic primitives for the GenICam Generic Control Protocol (GenCP).

Provides buffer builders, parsers, status codes, and command/acknowledgment helpers that work with any transport layer (GigE Vision UDP, USB3 Vision bulk, or custom).

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Command encoding** -- build GenCP request buffers (`ReadRegister`, `WriteRegister`, `ReadMem`, `WriteMem`)
- **Acknowledgment decoding** -- parse response buffers with status and payload extraction
- **Status codes** -- typed `StatusCode` enum (Success, NotImplemented, InvalidParameter, etc.)
- **Op codes & flags** -- `OpCode` enum and `CommandFlags` bitflags
- **Zero-copy** -- uses `bytes::Bytes` for efficient buffer handling

## Usage

```toml
[dependencies]
viva-gencp = "0.1"
```

```rust
use viva_gencp::{GenCpCmd, CommandHeader, OpCode, CommandFlags, encode_cmd};

let cmd = GenCpCmd {
    header: CommandHeader {
        flags: CommandFlags::ACK_REQUIRED,
        opcode: OpCode::ReadRegister,
        length: 8,
        request_id: 1,
    },
    payload: vec![0x00, 0x00, 0x0D, 0x00, 0x00, 0x00, 0x00, 0x04].into(),
};
let bytes = encode_cmd(&cmd);
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-gencp)

Part of the [genicam-rs](https://github.com/VitalyVorobyev/genicam-rs) workspace.
