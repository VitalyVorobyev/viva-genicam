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

## Saving and restoring a configuration

A user set lives in the camera. To keep a configuration on the host — to
version it, or to copy it onto another camera of the same model — save it to a
file:

```rust,ignore
{{#include ../../../crates/viva-genicam/examples/common_tasks.rs:save_load}}
```

What is saved is what the camera's XML marks `<Streamable>`. A feature behind
a selector — `TriggerMode` behind `TriggerSelector`, `Gain` behind
`GainSelector` — is saved once for each of the selector's entries, and the
selector is put back as it was. A feature that is not available, or locked,
is left out with a comment saying why; save while the camera is not acquiring,
or what acquisition locks (`Width`, `PixelFormat`) is left out.

A load writes in the order the camera needs, whatever the order of the file:
a selector before what it selects, `ExposureAuto` before the `ExposureTime` it
locks, `BinningHorizontal` before the `Width` it rescales. A write that fails
is tried once more after the others, because some features bound each other
(`Width` and `OffsetX`). Whatever still fails is in the report, with its
reason, and the rest is written. A file saved from another camera model loads
too, with a warning.

The file is text, one feature per line, in the layout GenApi's reference
implementation writes:

```text
# {05D8C294-F295-4dfb-9D01-096BD04049F4}
# ModelName = VivaCam Fake
Width	320
TriggerMode	{TriggerSelector=FrameStart}	Off
```

From the command line: `viva-camctl save --ip <CAMERA-IP> --out camera.txt`
and `viva-camctl load --ip <CAMERA-IP> --file camera.txt`
([the command line](../camctl.md)); from Python, `cam.save_features(path)` and
`cam.load_features(path)`.

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
