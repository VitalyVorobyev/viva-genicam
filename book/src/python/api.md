# API reference

Every public symbol exported from `viva_genicam`, plus the
`viva_genicam.testing` module.

```python
vg.__version__            # "X.Y.Z", the installed package version
```

## Discovery

```python
vg.discover(timeout_ms=500, iface=None, all=False) -> list[GigeDeviceInfo]
vg.discover_u3v() -> list[U3vDeviceInfo]
```

```python
@dataclass(frozen=True)
class GigeDeviceInfo:
    ip: str
    mac: str
    manufacturer: Optional[str]
    model: Optional[str]
    transport: Literal["gige"]

@dataclass(frozen=True)
class U3vDeviceInfo:
    bus: int
    address: int
    vendor_id: int
    product_id: int
    serial: Optional[str]
    manufacturer: Optional[str]
    model: Optional[str]
    transport: Literal["u3v"]
```

Both dataclasses expose `.to_dict()` for JSON-friendly export.

`DeviceInfo` is the `Union[GigeDeviceInfo, U3vDeviceInfo]` alias.

## Connection

```python
vg.connect_gige(device_info: GigeDeviceInfo, iface: Optional[str] = None) -> Camera
vg.connect_u3v(device_info: U3vDeviceInfo) -> Camera
vg.Camera.open(device_info, **kwargs) -> Camera   # dispatches on type
```

## Camera

```python
class Camera:
    transport: str                           # "gige" or "u3v"
    xml: str                                 # raw GenICam XML

    def get(self, name: str) -> str: ...
    def set(self, name: str, value: str) -> None: ...
    def set_exposure_time_us(self, value: float) -> None: ...
    def set_gain_db(self, value: float) -> None: ...
    def enum_entries(self, name: str) -> list[str]: ...

    def nodes(self) -> list[str]: ...
    def node_info(self, name: str) -> Optional[NodeInfo]: ...
    def all_node_info(self) -> list[NodeInfo]: ...
    def categories(self) -> dict[str, list[str]]: ...

    def execute(self, name: str) -> None: ...
    def acquisition_start(self) -> None: ...
    def acquisition_stop(self) -> None: ...

    def stream(
        self,
        iface: Optional[str] = None,
        packet_size: Optional[int] = None,
        auto_packet_size: bool = False,
        multicast: Optional[str] = None,
        destination_port: Optional[int] = None,
    ) -> FrameStream: ...
```

## NodeInfo

```python
class NodeKind(str, Enum):
    INTEGER       = "Integer"
    FLOAT         = "Float"
    ENUMERATION   = "Enumeration"
    BOOLEAN       = "Boolean"
    COMMAND       = "Command"
    CATEGORY      = "Category"
    SWISS_KNIFE   = "SwissKnife"
    CONVERTER     = "Converter"
    INT_CONVERTER = "IntConverter"
    STRING_REG    = "StringReg"
    REGISTER      = "Register"

@dataclass(frozen=True)
class NodeInfo:
    name: str
    kind: str                          # a NodeKind value
    access: Optional[str]              # declared: "RO" | "RW" | "WO" | None
    visibility: str                    # "Beginner" | "Expert" | "Guru" | "Invisible"
    display_name: Optional[str]
    description: Optional[str]
    tooltip: Optional[str]
    effective_access: Optional[str] = None   # live; set by node_info() only

    @property
    def readable(self) -> bool: ...    # from the declared access
    @property
    def writable(self) -> bool: ...
    def to_dict(self) -> dict: ...
```

`kind` is a plain string holding a `NodeKind` value. See
[Control & introspection → Node metadata](control.md#node-metadata) for what
`effective_access` means.

## FrameStream

```python
class FrameStream:
    def __enter__(self) -> "FrameStream": ...      # calls acquisition_start()
    def __exit__(self, *exc) -> None: ...           # calls acquisition_stop() + close()
    def __iter__(self) -> Iterator[Frame]: ...
    def __next__(self) -> Frame: ...                # 5-second default timeout
    def next_frame(self, timeout_ms: Optional[int] = None) -> Optional[Frame]: ...
    def close(self) -> None: ...
```

## Frame

```python
class Frame:
    width: int
    height: int
    pixel_format: str
    pixel_format_code: int
    ts_dev: Optional[int]
    ts_host: Optional[float]

    def payload(self) -> bytes: ...
    def to_numpy(self) -> numpy.ndarray: ...        # natural shape per pixel format
    def to_rgb8(self) -> numpy.ndarray: ...         # always (H, W, 3) uint8
```

## Testing

```python
from viva_genicam.testing import FakeGigeCamera

class FakeGigeCamera:
    def __init__(
        self,
        width: int = 640,
        height: int = 480,
        fps: int = 30,
        bind_ip: str = "127.0.0.1",
        port: int = 3956,
        pixel_format: str = "Mono8",
    ) -> None: ...
    def start(self) -> None: ...
    def stop(self) -> None: ...
    ip: str
    port: int
    def device_info(self, timeout_ms: int = 1500) -> GigeDeviceInfo: ...
    def __enter__(self) -> "FakeGigeCamera": ...    # calls start()
    def __exit__(self, *exc) -> None: ...            # calls stop()
```

An in-process fake GigE Vision camera, for tests and demos without hardware.
Keep `port=3956`: discovery only looks there, so `device_info()` and
`vg.connect_gige` cannot find a fake bound elsewhere, and only one fake per
host can hold that port at a time.

## Exceptions

```
GenicamError                      # base class
├── GenApiError
├── TransportError
├── ParseError
├── MissingChunkFeatureError
└── UnsupportedPixelFormatError
```

Everything the bindings raise deliberately inherits from `GenicamError`, so one
`except vg.GenicamError:` covers it.

It does **not** cover a `pyo3_runtime.PanicException`, which is what you get if
a `Camera`, `Frame` or `FrameStream` is used from a thread other than the one
that created it (they are pyo3 `unsendable` types), or if the Rust layer panics.
`PanicException` subclasses `BaseException`, so `except Exception:` misses it
too — see [Control & introspection](control.md#error-model).
