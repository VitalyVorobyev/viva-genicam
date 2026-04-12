# viva-sfnc

Standard Feature Naming Convention (SFNC) constants for GenICam.

Provides well-known feature name strings so you never have to hard-code `"ExposureTime"` or `"GainSelector"` in your application.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Features

- **Acquisition** -- `ACQUISITION_START`, `ACQUISITION_STOP`, `ACQUISITION_MODE`
- **Image control** -- `EXPOSURE_TIME`, `GAIN`, `GAIN_SELECTOR`, `PIXEL_FORMAT`
- **Chunk data** -- `CHUNK_MODE_ACTIVE`, `CHUNK_SELECTOR`, `CHUNK_ENABLE`
- **Events** -- `EVENT_SELECTOR`, `EVENT_NOTIFICATION`
- **Streaming** -- `STREAM_CH_SELECTOR`, `SCP_HOST_PORT`, `SCP_DEST_ADDR`, `MULTICAST_ENABLE`
- **Timestamps** -- `TS_LATCH_CMDS`, `TS_VALUE_NODES`, `TS_FREQ_NODES`
- **Multi-name support** -- vendor-agnostic arrays for features with alternative names

## Usage

```toml
[dependencies]
viva-sfnc = "0.1"
```

```rust
use viva_sfnc::{EXPOSURE_TIME, GAIN};

// Use constants instead of hard-coded strings
camera.set(EXPOSURE_TIME, "5000")?;
let gain = camera.get(GAIN)?;
```

## Documentation

[API reference (docs.rs)](https://docs.rs/viva-sfnc)

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.
