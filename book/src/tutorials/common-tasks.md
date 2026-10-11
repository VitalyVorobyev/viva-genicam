# Common tasks

The things nearly every camera application does — set the exposure and the
gain, trigger, crop, bin, fix the frame rate, save a configuration — have a
method each on `Camera`. They work with the names cameras actually use: a
camera that predates the current feature naming standard (SFNC 2) calls its
exposure `ExposureTimeAbs` or `ExposureTimeRaw` and its gain `GainRaw`, and
the same call handles it.

Everything here runs against the fake camera, no hardware needed:

```bash
cargo run -p viva-genicam --example common_tasks
cargo run -p viva-genicam --example common_tasks -- --legacy   # the older names
```

## Features by type

`get_integer`, `get_float`, `get_bool`, `get_enum`, `get_string` and their
`set_` counterparts read and write a feature as its own type. `get` and `set`
remain for text — a command line, a configuration file:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:typed}}
```

A category has no value and a command is not set: `get` on one, or `set` on
the other, is an error. Run a command with `execute_command`, or with
`execute_and_wait`, which also waits for the camera to report it finished.

## Exposure and gain

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:exposure_gain}}
```

Each helper tries the names in order and uses the first the camera declares,
then says which one that was, with its unit. `ExposureTime` and
`ExposureTimeAbs` are in microseconds and `Gain` in decibels; `ExposureTimeRaw`
and `GainRaw` are in the camera's own units, so the value you write means what
that camera's manual says it means.

## Region of interest and binning

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:roi_binning}}
```

A width's maximum shrinks as its offset grows, so the order of the writes
decides whether a region can be reached at all; `set_roi` gets it right
whichever way the region moves. A value off the feature's increment is refused
before anything is written, with the nearest values that would be accepted.

## Frame rate

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:frame_rate}}
```

`disable_frame_rate` lets the camera choose again.

## User sets

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:user_sets}}
```

`set_default_user_set` chooses the set the camera loads at power-up.

## Triggers

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:software_trigger}}
```

`acquisition_start` sets `TLParamsLocked` where the camera has it, so features
that would change the size of a frame — `Width`, `PixelFormat` — are locked
while it streams; `acquisition_stop` clears it.

## When the camera lacks a feature

A camera that declares none of a feature's names says so, naming every name
it tried:

```text
the camera has no exposure time: it declares none of ExposureTime, ExposureTimeAbs, ExposureTimeRaw
```

`camera.resolve(StandardFeature::ExposureTime)` asks without writing anything.

## Without an async runtime

A program with no async runtime uses `viva_genicam::blocking` to discover and
open a camera. The camera it returns is the ordinary `Camera`: everything
above works on it unchanged, and frames come from `next_frame_blocking`.

```bash
cargo run -p viva-genicam --example blocking_grab                 # the fake camera
cargo run -p viva-genicam --example blocking_grab -- 192.168.1.20  # a real one
```

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/blocking_grab.rs:discover}}
```

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/blocking_grab.rs:open}}
```

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/blocking_grab.rs:grab}}
```

The camera brings a small runtime of its own — one thread that keeps its
control channel alive — and stops it when the camera is dropped. The async
API works from a current-thread Tokio runtime as well.
