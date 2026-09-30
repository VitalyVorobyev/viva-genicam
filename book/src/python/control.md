# Control & introspection

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

### Typed helpers

Two SFNC-standard features have dedicated float setters so you don't pass numbers as strings:

```python
cam.set_exposure_time_us(10_000.0)
cam.set_gain_db(6.0)
```

These write the canonical SFNC names `ExposureTime` and `Gain` as floats, and
raise `GenApiError` if the camera calls them something else. They are a typed
convenience over `set`, not a compatibility layer — there is **no** vendor-alias
fallback here. If your camera uses a different name, pass it to `set` directly:

```python
cam.set("ExposureTimeAbs", "10000")
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
| `MissingChunkFeatureError` | Chunk selector not present in the camera's XML. Chunk configuration is not exposed to Python yet, so the bindings do not raise this today |
| `UnsupportedPixelFormatError` | No RGB conversion path for the reported pixel format |

All five inherit from `GenicamError`, so `except vg.GenicamError:` catches every
error the bindings *raise deliberately*. Two failures escape it:

- Using a `Camera`, `Frame` or `FrameStream` **from a thread other than the one
  that created it** raises `pyo3_runtime.PanicException`, which subclasses
  `BaseException` — so even `except Exception:` misses it. These objects are
  `unsendable`; keep each one on its own thread.
- A panic anywhere in the Rust layer surfaces the same way.

If you are wrapping this in a service that must not die, catch `BaseException`
at the top of the worker as well.

## Introspection

### List features

```python
cam.nodes()            # ['AcquisitionStart', 'ExposureTime', ...]
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
cam.set("UserSetSelector", "Default")
cam.execute("UserSetLoad")            # restore the camera's default settings
```

Commands carry no value; the camera acts on the write itself. `cam.get()` on a
Command raises, and there is nothing to read back.

Three things worth knowing:

- `cam.set("UserSetLoad", "1")` does the same thing — `set` dispatches
  Command nodes and discards the value. Prefer `execute`; it says what it
  does.
- **A read after the command returns the new value when the camera's XML says
  it should.** A register may declare `<pInvalidator>` nodes: "when this one
  changes, my cached value is stale". The FLIR Blackfly S BFS-PGE-31S4C-C names
  `UserSetLoad`'s register as an invalidator of 171 of its registers, and the
  features built on them — `ExposureTime` and `Gain` among them — go stale
  with them, so a `cam.get("ExposureTime")` after `cam.execute("UserSetLoad")`
  reads the camera again. A feature with no such link, direct or indirect,
  keeps its cached value; reconnect if you need one of those read back.

  Values the camera changes *on its own* — `ExposureTime` under auto exposure,
  say — are a different matter. GenICam governs those with `<Cachable>` and
  `<PollingTime>`, which are not honoured yet, so such a read can still be
  stale.

- **Separately**, GenICam's `pIsDone` polling is **not implemented**. `execute`
  returns when the register write is acknowledged, not when the camera has
  finished acting on it. That one a short sleep *does* help with.

From the command line:

```bash
viva-camctl execute --name UserSetLoad --ip <CAMERA-IP>
```

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

## Raw XML

```python
print(cam.xml[:500])     # first 500 chars of the GenICam XML
```

Handy for feeding into a GenICam tool, debugging a mystery feature, or archiving the exact schema a camera presented at connect time.

## Next

→ [Streaming](streaming.md) — sync iterator, NumPy frames, timestamps.
