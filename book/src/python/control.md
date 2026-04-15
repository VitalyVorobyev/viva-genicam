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

These resolve the canonical SFNC name (`ExposureTime` / `Gain`) and fall back to common vendor aliases; use them when you want to be resilient to small XML differences.

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
| `MissingChunkFeatureError` | Chunk selector not present in the camera's XML |
| `UnsupportedPixelFormatError` | No RGB conversion path for the reported pixel format |

All inherit from `GenicamError` so `except vg.GenicamError:` catches everything.

## Introspection

### List features

```python
cam.nodes()            # ['AcquisitionStart', 'ExposureTime', ... 53 entries]
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
```

`NodeInfo` fields:

- `name` — feature name
- `kind` — `"Integer"`, `"Float"`, `"Enumeration"`, `"Boolean"`, `"Command"`, `"Category"`, `"SwissKnife"`, `"Converter"`, `"IntConverter"`, `"StringReg"`
- `access` — `"RO"`, `"RW"`, `"WO"`, or `None` (for categories)
- `visibility` — `"Beginner"`, `"Expert"`, `"Guru"`, `"Invisible"`
- `display_name`, `description`, `tooltip`

Plus two convenience properties: `readable` (`access in {"RO","RW"}`) and `writable` (`access in {"RW","WO"}`).

### Enum entries

```python
cam.enum_entries("PixelFormat")
# ['Mono8', 'Mono16', 'BayerRG8', 'RGB8Packed']
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
