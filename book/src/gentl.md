# Use viva from other tools

Much machine-vision software does not talk to cameras itself: it loads a
camera driver written to the EMVA's **GenTL** standard, a *GenTL producer* — a
shared library with the extension `.cti`. viva-genicam ships one,
`viva_gentl.cti`, so that software can find, configure and stream from GigE
Vision cameras through viva, with no vendor SDK.

[Harvesters](https://github.com/genicam/harvesters), the Python GenTL
consumer, is tested against it on every change. No other consumer has been
tried yet: if you use one, [tell us](reporting.md) how it went.

## What it offers

- **GigE Vision cameras.** USB3 Vision cameras are not offered through it yet.
- **One interface per network interface** of the host, loopback included, so
  the [fake camera](tutorials/fake-camera.md) shows up too.
- **Read-only, control and exclusive access.** Read-only access watches a
  camera that another application controls.
- **One data stream per camera**, into buffers your software allocates or the
  producer allocates. A frame with lost packets is delivered and marked
  incomplete; chunk data is located in the buffer.
- **The camera's own features**, from its XML, through the consumer's GenApi.

## Get the `.cti`

Releases from 0.9.0 attach one per platform to the
[GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases):

| OS | File |
|---|---|
| Linux x86_64 | `viva_gentl-x86_64-unknown-linux-gnu.cti` |
| macOS, Apple Silicon | `viva_gentl-aarch64-apple-darwin.cti` |
| Windows x86_64 | `viva_gentl-x86_64-pc-windows-msvc.cti` |

They are 64-bit only. The Linux build needs glibc 2.39 (Ubuntu 24.04) or
newer. The files are not signed: macOS may refuse to load one downloaded with a
browser until its quarantine flag is cleared:

```bash
xattr -d com.apple.quarantine viva_gentl-aarch64-apple-darwin.cti
```

To build it from source instead, with Rust installed, from a clone of the
repository:

```bash
scripts/build-cti.sh          # prints target/gentl/viva_gentl.cti
```

## Point your software at it

GenTL consumers look for producers in the directories listed in
`GENICAM_GENTL64_PATH`, separated by `:` on Linux and macOS and by `;` on
Windows (GenTL 1.6 §6.1.1). Put the `.cti` in a directory of its own and add
that directory:

```bash
export GENICAM_GENTL64_PATH="$HOME/viva-gentl:$GENICAM_GENTL64_PATH"
```

```powershell
$env:GENICAM_GENTL64_PATH = "C:\viva-gentl;$env:GENICAM_GENTL64_PATH"
```

Some consumers take the file directly instead; Harvesters is one.

## Harvesters

`pip install harvesters` installs Harvesters and the EMVA's GenApi. Load the
`.cti`, list the cameras it finds, and open one by any of the fields it lists
(`model`, `serial_number`, `id_`, …):

```python
from harvesters.core import Harvester

{{#include ../../scripts/gentl-harvesters.py:open}}
```

```python
harvester, acquirer = open_camera("viva_gentl.cti", "FakeGigE")
```

The camera's features are on `acquirer.remote_device.node_map` — for example
`acquirer.remote_device.node_map.ExposureTime.value`. Start acquisition with
`acquirer.start()`, then fetch frames:

```python
{{#include ../../scripts/gentl-harvesters.py:fetch}}
```

`acquirer.stop()` ends acquisition; `acquirer.destroy()` and
`harvester.reset()` close the camera and unload the producer.

The two functions come from the script that CI runs against the fake camera,
[`scripts/gentl-harvesters.py`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/scripts/gentl-harvesters.py).
To try them without a camera, start the fake first (`viva-fake-gige`, see
[Testing without hardware](tutorials/fake-camera.md)); its model is
`FakeGigE`.

## When something goes wrong

Set `RUST_LOG=debug` before starting your software and the producer logs to
standard error, as the rest of viva does. Every failed call also leaves a
message, which a consumer reads with `GCGetLastError`; whether and how it shows
it depends on the consumer. If a camera works in `viva-camctl` but not through
your software, [report it](reporting.md) with the name and version of that
software.
