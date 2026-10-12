# Control & introspection

The examples use `info`, a `GigeDeviceInfo` from `vg.discover()`, and `cam`,
the camera `vg.connect_gige(info)` opened.

## Reading and writing features

`Camera.get(name)` returns the value as a string, formatted per the node's type. `Camera.set(name, value)` parses the string according to the node type and writes it.

```python
cam.get("ExposureTime")         # "5000"
cam.get("PixelFormat")          # "Mono8"
cam.get("Width")                # "640"

cam.set("Width", "320")
cam.set("PixelFormat", "Mono8")
cam.set("ExposureTime", "7500.0")
```

### Reading and writing by type

Each node type has its own pair, which skips the round trip through text and
returns a Python value of the feature's type:

<!-- pytest: camera -->
```python
cam.set_integer("Width", 320)
assert cam.get_integer("Width") == 320
cam.set_float("ExposureTime", 7500.0)
cam.set_bool("AcquisitionFrameRateEnable", False)
cam.set_enum("AcquisitionMode", "Continuous")
print(cam.get_string("DeviceModelName"))
```

`get_register`/`set_register` read and write a `<Register>` feature's bytes;
`set_register` takes exactly its length. A type that does not match the
feature — `get_integer` on a `Float` — raises `GenApiError`.

### Standard features

Exposure, gain, triggers, the region of interest, binning, frame rate and user
sets have helpers that find the feature the camera has for them — SFNC's name,
or an older one such as `ExposureTimeAbs` or `GainRaw` — and raise
`GenicamError`, naming every name tried, when it has none:

<!-- pytest: camera -->
```python
cam.set_exposure_time(10_000.0)   # returns the feature written: "ExposureTime"
cam.set_gain(6.0)                 # "Gain", or "GainRaw" on an older camera
cam.set_roi(vg.Roi(x=0, y=0, width=320, height=240))
cam.set_binning(1, 1)

cam.enable_trigger("FrameStart", "Software")
cam.trigger_software()            # one frame
cam.disable_trigger("FrameStart")

cam.save_user_set("UserSet1")
cam.load_user_set("UserSet1")
cam.set_roi(vg.Roi(0, 0, 640, 480))
```

The exposure goes to `ExposureTime`, else `ExposureTimeAbs` (both in
microseconds), else `ExposureTimeRaw` (the camera's own units, rounded to an
integer); the gain to `Gain` (decibels), else `GainRaw` (the camera's own
units). Each setter has a getter — `exposure_time()`, `gain()`, `roi()`,
`binning()`, `frame_rate()`, `acquisition_mode()`, `user_sets()` — and
`set_frame_rate(hz)` switches the camera's frame-rate enable on first, where
it has one.

### One feature's whole state

`feature_state(name)` reads what a viewer shows, at once: the typed value,
what the camera permits now, whether the feature is implemented and
available, its range, the entries an enumeration accepts now, its unit, and
how often to read it again:

<!-- pytest: camera -->
```python
state = cam.feature_state("DeviceTemperature")
print(state.value, state.unit)   # e.g. 40.25 C
print(state.polling_time)        # 0.5: read it again every half second
print(state.caching)             # "NoCache": the camera changes it on its own
```

`available_enum_entries(name)` is the list an enumeration accepts now, after
each entry's own `pIsImplemented`/`pIsAvailable`; `enum_entries(name)` lists
every entry the XML declares.

### Values that change

A value the camera changes by itself — a temperature, an exposure under auto
exposure — is read from the camera again when the XML says so: a feature
declared `NoCache` every time, one with a `PollingTime` once that has passed.
Two calls tell a viewer what to read again:

<!-- pytest: camera -->
```python
seen = cam.generation()
# ... writes, here or from another thread ...
changed, seen = cam.changed_since(seen)   # features written or invalidated since
due = cam.poll()                          # features whose PollingTime has passed
```

### Error model

Every control error raises a subclass of `vg.GenicamError`:

```python
try:
    cam.set("Width", "not-a-number")
except vg.ParseError as e:
    print("bad input:", e)
except vg.GenApiError as e:
    print("nodemap rejected the write:", e)
except vg.TransportError as e:
    print("register I/O failed:", e)
```

| Exception | When |
|---|---|
| `GenApiError` | Nodemap evaluation: unknown feature, value out of range, predicate failed |
| `TransportError` | GVCP/USB register read or write failed |
| `ParseError` | User-supplied value couldn't be parsed per the node's type |
| `MissingChunkFeatureError` | `configure_chunks()` on a camera whose XML has no chunk mode, selector or enable |
| `UnsupportedPixelFormatError` | No RGB conversion path for the reported pixel format |

All five inherit from `GenicamError`, so `except vg.GenicamError:` catches every
error the bindings raise deliberately; a closed camera, and a GigE Vision call
on a USB3 Vision camera, raise `GenicamError` itself. A panic in the Rust layer
would surface as `pyo3_runtime.PanicException`, a `BaseException` that
`except Exception:` misses: a service that must not die catches `BaseException`
at the top of its worker as well.

## Threads

A `Camera`, a `Frame` and a stream can be used from any thread. Calls to one
camera from several threads take turns; every call that waits on the camera —
a register read, a frame, an event — releases the GIL, so other Python threads
run meanwhile.

## Closing a camera

A GigE camera holds the camera's control privilege until it is closed, and
another application — or a second run of this one — cannot take control
meanwhile. Close it with `close()`, or use the camera as a context manager:

<!-- pytest: device -->
```python
with vg.connect_gige(info) as cam:
    print(cam.get("Width"))
# control is given back here; `cam` raises GenicamError from now on
```

Without either, control is given back when the camera is garbage-collected.

## Control privilege

`control_state` says where a GigE session stands: `HELD`, `MONITOR` (opened
with `monitor=True`), `LOST` (the camera timed the heartbeat out, or gave
control to another application) or `RELEASED`. Only `HELD` writes; in every
other state a write raises before anything is sent.

<!-- pytest: camera -->
```python
cam.release_control()          # let another application take the camera
assert cam.control_state is vg.ControlState.RELEASED
cam.reclaim_control()          # raises while someone else holds it
assert cam.control_state is vg.ControlState.HELD
```

## Introspection

### List features

```python
cam.nodes()            # ['AcquisitionStart', 'ExposureTime', ...]
```

A feature you expected but cannot find may be one the library could not build
from the camera's XML rather than one the camera lacks. `skipped()` lists those,
with the reason — worth an issue with the camera's XML attached:

```python
for node in cam.skipped():   # [] for a document the library fully understands
    print(node.tag, node.name, node.error)
```

### Node metadata

```python
info = cam.node_info("ExposureTime")
print(info.kind)         # "Float"
print(info.access)       # "RW"
print(info.visibility)   # "Beginner"
print(info.description)  # "Exposure time of the sensor in microseconds."
print(info.writable)     # True
print(info.readable)     # True
print(info.effective_access)  # "RW" — or "RO" while the feature is locked
```

`NodeInfo` fields:

- `name` — feature name
- `kind` — `"Integer"`, `"Float"`, `"Enumeration"`, `"Boolean"`, `"Command"`, `"Category"`, `"SwissKnife"`, `"Converter"`, `"IntConverter"`, `"StringReg"`, `"Register"`
- `access` — the access mode the XML declares: `"RO"`, `"RW"`, `"WO"`, or `None` (for categories)
- `visibility` — `"Beginner"`, `"Expert"`, `"Guru"`, `"Invisible"`
- `display_name`, `description`, `tooltip`
- `effective_access` — the access mode the camera permits *right now*: a
  `"RW"` feature reads as `"RO"` while a `pIsLocked` lock is engaged or while
  the feature is unavailable. Only `node_info()` fills it in, and leaves it
  `None` if the predicates cannot be evaluated; `all_node_info()` always
  leaves it `None`, because resolving it costs register reads per node.

Plus two convenience properties, both computed from the declared `access`:
`readable` (`access in {"RO","RW"}`) and `writable` (`access in {"RW","WO"}`).

### Enum entries

```python
cam.enum_entries("PixelFormat")
# ['Mono8', 'Mono16', 'BayerRG8', 'RGB8Packed']
```

### Executing commands

Some features are *actions*, not values — `UserSetLoad`, `TimestampLatch`,
`TriggerSoftware`. `node_info(name).kind == "Command"` identifies one, and
`execute` runs it:

```python
cam.set("UserSetSelector", "UserSet1")
cam.execute("UserSetLoad")            # load user set 1, and wait until it is loaded
```

Commands carry no value of their own; the camera's XML says what executing one
writes. Often it is a constant. Sometimes it is another feature's value: on a
Hikrobot MV-CS050, `UserSetSave` and `UserSetLoad` write the selected set's
number, so the set you select is the set that is saved or loaded. `cam.get()`
on a Command raises, and there is nothing to read back.

`execute` **waits until the camera reports the command finished** — GenICam's
`IsDone` — for up to `timeout` seconds (5 by default), and returns `True` once
it does. If the camera has not reported completion by then, `execute` emits a
`RuntimeWarning` and returns `False`; the command was still sent. Most commands
write a write-only register and count as finished as soon as the write is
acknowledged. A self-clearing one is polled until it clears.
`execute(name, wait=False)` returns `False` once the write is acknowledged,
without asking.

A command can be refused before anything is sent: Hikrobot's `UserSetSave`
is unavailable while `Default` is selected, and `execute` raises
`GenApiError` rather than writing.

Two more things worth knowing:

- A command is executed, never set: `cam.set("UserSetLoad", "1")` raises
  `GenApiError`.
- **A read after the command returns the new value when the camera's XML says
  it should.** A register may declare `<pInvalidator>` nodes: "when this one
  changes, my cached value is stale". The FLIR Blackfly S BFS-PGE-31S4C-C names
  `UserSetLoad`'s register as an invalidator of 171 of its registers, and the
  features built on them — `ExposureTime` and `Gain` among them — go stale
  with them, so a `cam.get("ExposureTime")` after `cam.execute("UserSetLoad")`
  reads the camera again. A feature with no such link, direct or indirect,
  keeps its cached value; `cam.invalidate(name)` (or `cam.invalidate_all()`)
  makes the next read ask the camera.

  Values the camera changes *on its own* — a temperature, `ExposureTime`
  under auto exposure — need none of this when the XML says so: a feature
  declared `<Cachable>NoCache</Cachable>` is read from the camera every time,
  and one with a `<PollingTime>` once that has passed.

From the command line, which waits the same way (`--wait-ms` bounds it,
`--no-wait` skips it):

```bash
viva-camctl execute --name UserSetLoad --ip <CAMERA-IP>
```

### Saving and restoring a configuration

```python
saved = cam.save_features("camera.txt")      # {"values": 42, "left_out": [...]}
report = cam.load_features("camera.txt")
for failure in report["failures"]:
    print("not written:", failure)
```

What is saved, the order of the writes, and the file are described in
[Common tasks](../tutorials/common-tasks.md#saving-and-restoring-a-configuration).

### Categories

```python
cats = cam.categories()
for cat, children in cats.items():
    print(cat, "->", children)
```

The categories map mirrors the GenICam XML category tree; each value is the list of child feature names. Use this to render a tree UI or to filter features by area (acquisition, image format, device control, etc.).

### All node metadata at once

```python
for info in cam.all_node_info():
    print(info.name, info.kind, info.access)
```

Useful for exporting a CSV, auto-generating GUI forms, or diffing two cameras' feature surfaces.

## Acquisition control

Without streaming (for example, trigger-mode tests):

```python
cam.acquisition_start()
# ... do something that causes frames to be produced on another channel ...
cam.acquisition_stop()
```

When you use `with cam.stream() as frames:` the stream context manager calls these for you on entry/exit. Don't call them manually if you are using `stream()`.

## Raw registers

`read_register(address, length=4)` and `write_register(address, data)` reach
the camera's register space past GenApi — a bootstrap register, or one the XML
does not name. A raw write forgets every cached value, since it may have
changed any of them.

<!-- pytest: camera -->
```python
ccp = cam.read_register(0x0A00)   # GigE Vision's control channel privilege
print(ccp.hex())                  # "00000002": this application has control
```

## Action commands and IP configuration

An action command triggers every camera whose keys match at once —
`ActionDeviceKey`, `ActionGroupKey`, `ActionGroupMask` — and each that acts
acknowledges:

<!-- pytest: device -->
```python
summary = vg.send_action(device_key=0x42, group_key=1, group_mask=1, destination=info.ip)
print(summary.acks, "camera(s) acted")   # 1 when the keys are the camera's
```

Without `destination=` the command goes to the broadcast address, so every
camera on the subnet hears it; `scheduled_time=` (in the cameras' timestamp
ticks) asks them to act then.

A camera configured for another network still answers discovery, at an address
the host cannot reach. `vg.force_ip(mac, ip, subnet)` gives it a temporary
address, until it restarts; `cam.set_persistent_ip(ip, subnet, gateway)` stores
one in a camera you have opened, for its next power-up. `viva-camctl set-ip`
does the same from the command line.

## Raw XML

```python
print(cam.xml[:500])     # first 500 chars of the GenICam XML
```

Handy for feeding into a GenICam tool, debugging a mystery feature, or archiving the exact schema a camera presented at connect time.

## Next

→ [Streaming](streaming.md) — sync iterator, NumPy frames, timestamps.
