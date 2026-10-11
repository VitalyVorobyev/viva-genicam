# viva-gencp

Control-protocol primitives shared by the GigE Vision and USB3 Vision transports.

GigE Vision's GVCP and GenCP (which USB3 Vision carries) share ideas and a core
of status codes, but not their bytes on the wire. This crate holds both
vocabularies: GVCP's opcodes and headers, GenCP's command ids and flags, and
the status codes the two share.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Acknowledgment decoding** -- `decode_ack` parses a GVCP acknowledgement: status, request id and payload
- **Status codes** -- typed `StatusCode` enum (Success, NotImplemented, InvalidParameter, etc.)
- **Op codes & flags** -- GVCP's `OpCode` and `CommandFlags`; GenCP's `command_id` and `ccd_flags`
- **Zero-copy** -- uses `bytes::Bytes` for efficient buffer handling

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-gencp)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
