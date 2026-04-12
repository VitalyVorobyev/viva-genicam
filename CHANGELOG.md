# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-04-12

### Added

- **Multi-platform release binaries** -- release workflow now produces prebuilt `viva-camctl` and `viva-service` archives for Linux x86_64, macOS x86_64, macOS aarch64, and Windows x86_64; each archive bundles the binaries with `README.md`, `LICENSE`, and `CHANGELOG.md`, and a `SHA256SUMS.txt` is published alongside
- **`viva-camctl` on crates.io** -- the CLI is now published, so `cargo install viva-camctl` works

### Changed

- Internal workspace dependency version requirements simplified from `"0.2.0"` to `"0.2"` (semver-equivalent, but avoids a sweep on every patch bump)
- Release workflow dropped the redundant `.crate` packaging step -- those archives are hosted on crates.io via the publish-crates workflow

### Fixed

- `GigeRegisterIo` now detects async context via `Handle::try_current()` and wraps `block_on` in `tokio::task::block_in_place` only when inside a runtime, preventing nested-runtime panics while preserving plain synchronous usage

## [0.2.0] - 2026-04-11

### Added

- **USB3 Vision streaming** -- `U3vFrameStream` async frame iterator wrapping blocking bulk reads via `spawn_blocking`, `U3vStreamBuilder` for configuring U3V streams through the same pattern as GigE
- **USB3 Vision service** -- `viva-service-u3v` now supports real USB cameras (previously `--fake` only); `U3vDeviceHandle` is generic over `T: UsbTransfer`
- **USB3 Vision CLI** -- `viva-camctl stream-usb` command for frame streaming from USB3 Vision cameras
- **FORCEIP command** -- GVCP opcode 0x0004 for temporary IP assignment via broadcast (targets device by MAC address)
- **Persistent IP configuration** -- read/write bootstrap registers for persistent IP, subnet, and gateway; `enable_persistent_ip()` method on `GigeDevice`
- **IP configuration CLI** -- `viva-camctl set-ip` command with `--force` (FORCEIP) and persistent register modes
- **Reconnection with backoff** -- `DeviceHandle::refresh_connection()` retries up to 5 times with exponential backoff (500ms base, 16s max)
- **GenApi node metadata** -- `NodeMeta` struct with `Visibility`, `Description`, `ToolTip`, `DisplayName`, `Representation` fields; parsed from XML and exposed on all node types
- **Visibility filtering** -- `Visibility` enum (Beginner/Expert/Guru/Invisible), `Representation` enum (Linear/Logarithmic/HexNumber/etc.), `NodeMap::nodes_at_visibility()` for UI filtering
- **`U3vDevice::transport()`** -- public accessor for the shared USB transport `Arc<T>`
- **Bayer 16-bit pixel formats** -- `PixelFormat` enum now includes BayerGR16, BayerRG16, BayerGB16, BayerBG16 with correct PFNC codes; `PixelFormat::from_name()` for string-to-enum conversion
- **`PixelFormat::from_name()`** -- parse PFNC name strings (e.g. "RGB8", "Mono16", "BayerRG16") to `PixelFormat`

### Changed

- MSRV raised from 1.85 to 1.88 (resolves `time` crate security advisory RUSTSEC-2026-0009)
- Project tagline updated from "Ethernet-first" to "GigE Vision and USB3 Vision" reflecting dual-transport support
- `viva-service-u3v` Cargo.toml now enables `u3v-usb` feature for real USB support
- Added `cargo deny check advisories` to CI pipeline with `deny.toml` allow-list for zenoh transitive advisories

### Fixed

- GitHub Pages deployment error ("Tag v0.1.0 not allowed to deploy") by removing wildcard tag trigger from `publish-docs.yml`
- SVG logo dot alignment and genicam text spacing for correct browser rendering
- `time` crate DoS vulnerability (RUSTSEC-2026-0009) by upgrading to 0.3.47

### Known Issues

- `lz4_flex 0.10.0` (RUSTSEC-2026-0041, high) and `rsa 0.9.10` (RUSTSEC-2023-0071, medium) are transitive dependencies through `zenoh 1.9.0` and cannot be updated until zenoh releases a fix. Neither is exploitable through our usage (lz4 decompression of untrusted data, RSA timing attack). Tracked in `deny.toml`.

## [0.1.0] - 2026-04-10

Initial public release of the genicam-rs workspace.

### Added

- **viva-genicam** -- High-level facade crate with `Camera<T>`, discovery, streaming, events, and action commands
- **viva-gige** -- GigE Vision transport layer: GVCP discovery, GenCP register I/O, GVSP streaming with resend and reassembly
- **viva-genapi** -- In-memory GenApi node map with typed feature access (Integer, Float, Enum, Boolean, Command, SwissKnife, Converter, String)
- **viva-genapi-xml** -- GenICam XML parsing into an intermediate representation with async XML fetch
- **viva-gencp** -- Transport-agnostic GenCP protocol encode/decode
- **viva-u3v** -- USB3 Vision transport: bootstrap registers, GenCP-over-USB control, and bulk streaming
- **viva-pfnc** -- Pixel Format Naming Convention (PFNC) tables and helpers
- **viva-sfnc** -- Standard Feature Naming Convention (SFNC) string constants
- **viva-zenoh-api** -- Shared Zenoh API payload types (no Zenoh dependency)
- **viva-service** -- Zenoh bridge exposing GenICam cameras as network services

### Protocol Features

- GVCP discovery (broadcast and unicast)
- GenCP register read/write with retry and backoff
- GVSP streaming with frame reassembly
- Packet resend with bitmap tracking and exponential backoff
- Automatic packet size negotiation from MTU
- Multicast stream support (IGMP join/leave)
- GVCP event channel with timestamp mapping
- Action commands with scheduled execution
- Chunk data parsing (timestamp, exposure time, gain, line status)
- Extended ID support (64-bit block IDs, 32-bit packet IDs per GigE Vision 2.0+)

### Testing

- `viva-fake-gige` -- In-process fake GigE Vision camera for self-contained integration testing (no external dependencies required)
- `viva-fake-u3v` -- In-process fake USB3 Vision camera for testing

[0.2.1]: https://github.com/VitalyVorobyev/genicam-rs/releases/tag/v0.2.1
[0.2.0]: https://github.com/VitalyVorobyev/genicam-rs/releases/tag/v0.2.0
[0.1.0]: https://github.com/VitalyVorobyev/genicam-rs/releases/tag/v0.1.0
