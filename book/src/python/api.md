# API reference

Every public symbol exported from `viva_genicam`.

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

    def acquisition_start(self) -> None: ...
    def acquisition_stop(self) -> None: ...

    def stream(
        self,
        iface: Optional[str] = None,
        auto_packet_size: Optional[bool] = None,
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

@dataclass(frozen=True)
class NodeInfo:
    name: str
    kind: str
    access: Optional[str]              # "RO" | "RW" | "WO" | None
    visibility: str                    # "Beginner" | "Expert" | "Guru" | "Invisible"
    display_name: Optional[str]
    description: Optional[str]
    tooltip: Optional[str]

    @property
    def readable(self) -> bool: ...
    @property
    def writable(self) -> bool: ...
    def to_dict(self) -> dict: ...
```

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

## Exceptions

```
GenicamError                      # base class
├── GenApiError
├── TransportError
├── ParseError
├── MissingChunkFeatureError
└── UnsupportedPixelFormatError
```

All raised by the camera / frame APIs inherit from `GenicamError`, so one `except vg.GenicamError:` catches every bindings-level failure.
