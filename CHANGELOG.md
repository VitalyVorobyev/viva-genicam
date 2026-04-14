# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.4] - 2026-04-13

### Added

- **Python bindings (`viva-genicam` on PyPI)** -- new `crates/viva-pygenicam` PyO3 crate plus pure-Python facade in `python/viva_genicam/`. Ships as an abi3 wheel covering discovery, control, introspection, and streaming for both GigE Vision and USB3 Vision cameras. Frames expose a NumPy-friendly `to_numpy()` / `to_rgb8()` API, streams are sync iterators over a managed Tokio runtime (no asyncio required), errors map onto a `GenicamError` subclass hierarchy, and `py.typed` + `.pyi` stubs give IDEs full completion. Fake-camera pytest suite (19 tests) runs against the built wheel.
- **Python wheels CI (`.github/workflows/python.yml`)** -- cross-platform wheel matrix (Linux x86_64, macOS arm64, Windows x86_64) × Python 3.9–3.13; libusb is statically vendored into the extension so no system libusb is required by the wheel. Publishes to PyPI via OIDC on `py-v*` tags. Test install uses `pip --no-index --find-links dist` so CI never falls back to a published PyPI wheel while validating the just-built artifact.
- **Auto-detected GigE streaming interface** -- `camera.stream()` now picks the NIC whose IPv4 subnet contains the camera's IP when `iface` is omitted; loopback cameras resolve to `lo`/`lo0` automatically. An explicit `iface=` override remains available on both `connect_gige` and `stream`.
- **`book/src/python.md`** -- Python API tutorial chapter; README gains a Python section.
- **Python examples** -- `crates/viva-pygenicam/examples/` ships five runnable scripts (`discover.py`, `get_set_feature.py`, `node_browser.py`, `grab_frame.py`, `demo_fake_camera.py`) plus an `examples/README.md` that describes each.
- **Expanded book tutorial** -- `book/src/python.md` becomes an index; five sibling pages under `book/src/python/` walk through install, discovery, control & introspection, streaming, and a full API reference.
- **In-process fake camera (`viva_genicam.testing.FakeGigeCamera`)** -- the `viva-fake-gige` crate is now bound as a Python class shipped inside the wheel. `pip install viva-genicam` alone is enough to run the full demo end-to-end; no subprocess, no binary to build, no repo clone required. The `demo_fake_camera.py` example and `tests/conftest.py` were migrated onto the in-process path.

### Changed

- Root `Cargo.toml` gains `workspace.exclude = ["crates/viva-pygenicam"]` so PyO3/maturin stays out of the default `cargo test --workspace` path.

## [0.2.3] - 2026-04-12

### Added

- **Zenoh API v2 `FeatureState` contract** -- new `FeatureState`, `NumericRange`, and `CommandResult` wire types expose live feature introspection (is_implemented / is_available / access_mode / numeric range / enum_available) per node; `introspect` queryable wired into `viva-service` and `viva-service-u3v`. Legacy `NodeValueUpdate` stays wire-compatible. See ADR-010.
- **GenApi predicate evaluation** -- `NodeMap::is_implemented`, `is_available`, `effective_access_mode`, and `available_enum_entries` evaluate `pIsImplemented` / `pIsAvailable` / `pIsLocked` / per-enum-entry predicates via the existing `resolve_numeric` machinery (Integer / Boolean / SwissKnife / IntConverter / Converter / Enum providers all supported, with cycle detection)
- **Predicate refs on every `NodeDecl` variant** -- `PredicateRefs` (`p_is_implemented` / `p_is_available` / `p_is_locked`) parsed from `<pIsImplemented>` / `<pIsAvailable>` / `<pIsLocked>` and plumbed through `NodeMap::try_from_xml` with proper dependency registration; `Node::predicates()` exposes the refs for external evaluators
- **Realistic predicate wiring in the fake GigE camera** -- `ExposureTime.pIsLocked` ← `ExposureAuto != Off`, `Gain.pIsLocked` ← `GainAuto != Off`, `AcquisitionFrameRate.pIsAvailable` ← new `AcquisitionFrameRateEnable` Boolean, `PixelFormat` entries gated by a new `SensorType` enum (Monochrome / BayerRG / Color)

### Changed

- **`DeviceHandle::get_feature_state`** now reports live `is_implemented` / `is_available` / `access_mode` / `enum_available` from the predicate evaluators instead of hardcoded permissive defaults; each predicate call is guarded so a single bad formula doesn't break the whole feature snapshot

### Fixed

- **Float bit-pattern bug** -- `<FloatReg>` and bare `<Float>` with `Length in {4, 8}` and no `<Scale>`/`<Offset>` now auto-infer IEEE 754 encoding. Before this fix, `AcquisitionFrameRate` came back as `1106247680` (the f32 bit pattern of 30.0) and `ExposureTime` as `4662219572839973000` because float registers were always read as scaled i64. New `FloatEncoding` (Ieee754 / ScaledInteger) + `byte_order` on `NodeDecl::Float`; `get_float` / `set_float` dispatch on encoding.

## [0.2.2] - 2026-04-12

### Changed

- Rename old repo name `viva-genicam` to the new `viva-genicam`

## [0.2.1] - 2026-04-12

### Added

- **Multi-platform release binaries** -- release workflow now produces prebuilt `viva-camctl` and `viva-service` archives for Linux x86_64, macOS aarch64 (Apple Silicon), and Windows x86_64; each archive bundles the binaries with `README.md`, `LICENSE`, and `CHANGELOG.md`, and a `SHA256SUMS.txt` is published alongside
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

Initial public release of the viva-genicam workspace.

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

[0.2.3]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.3
[0.2.2]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.2
[0.2.1]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.1
[0.2.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.2.0
[0.1.0]: https://github.com/VitalyVorobyev/viva-genicam/releases/tag/v0.1.0
