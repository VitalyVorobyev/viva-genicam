# API reference

Every public symbol exported from `viva_genicam`, plus the
`viva_genicam.testing` module.

```python
vg.__version__            # "X.Y.Z", the installed package version
```

## Discovery

```python
vg.discover(timeout_ms=500, iface=None, all=False, allow_broadcast_ack=False) -> list[GigeDeviceInfo]
vg.discover_ip(ip: str, timeout_ms=500) -> Optional[GigeDeviceInfo]
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
    bus: str                    # the OS's bus identifier; its form varies by OS
    address: int
    vendor_id: int
    product_id: int
    serial: Optional[str]
    manufacturer: Optional[str]
    model: Optional[str]
    speed: Optional[str]        # e.g. "high speed (480 Mbit/s, USB 2)"
    transport: Literal["u3v"]
```

Both dataclasses expose `.to_dict()` for JSON-friendly export.

`DeviceInfo` is the `Union[GigeDeviceInfo, U3vDeviceInfo]` alias.

## Connection

```python
vg.connect_gige(device_info: GigeDeviceInfo, iface: Optional[str] = None, monitor: bool = False, exclusive: bool = False) -> Camera
vg.connect_u3v(device_info: U3vDeviceInfo) -> Camera
vg.Camera.open(device_info, **kwargs) -> Camera   # dispatches on type
```

`monitor=True` opens a GigE camera read-only, alongside the application that
controls it; `exclusive=True` claims exclusive access, which keeps every other
application from reading it too ([Networking → One camera, several applications](../networking.md#61-one-camera-several-applications)).

## Camera

Any thread may use a camera; every call that waits on it releases the GIL.
Methods marked *GigE* raise `GenicamError` on a USB3 Vision camera.

```python
class Camera:
    transport: str                           # "gige" or "u3v"
    xml: str                                 # raw GenICam XML
    closed: bool
    control_state: ControlState              # GigE

    # Lifetime: close() gives control privilege back; so does leaving `with`.
    def close(self) -> None: ...
    def __enter__(self) -> "Camera": ...
    def __exit__(self, *exc) -> None: ...

    # Features as text, and by type.
    def get(self, name: str) -> str: ...
    def set(self, name: str, value: str) -> None: ...
    def get_integer(self, name: str) -> int: ...
    def set_integer(self, name: str, value: int) -> None: ...
    def get_float(self, name: str) -> float: ...
    def set_float(self, name: str, value: float) -> None: ...
    def get_bool(self, name: str) -> bool: ...
    def set_bool(self, name: str, value: bool) -> None: ...
    def get_enum(self, name: str) -> str: ...
    def set_enum(self, name: str, entry: str) -> None: ...
    def get_string(self, name: str) -> str: ...
    def set_string(self, name: str, value: str) -> None: ...
    def get_register(self, name: str) -> bytes: ...
    def set_register(self, name: str, data: bytes) -> None: ...
    def feature_state(self, name: str) -> FeatureState: ...
    def enum_entries(self, name: str) -> list[str]: ...            # declared
    def available_enum_entries(self, name: str) -> list[str]: ...  # accepted now

    # What changed: every write and invalidation advances the generation.
    def generation(self) -> int: ...
    def changed_since(self, since: int) -> tuple[list[str], int]: ...
    def poll(self) -> list[str]: ...           # values whose PollingTime passed
    def invalidate(self, name: str) -> None: ...
    def invalidate_all(self) -> None: ...

    # Standard features, by SFNC's names or the older ones cameras use.
    def exposure_time(self) -> float: ...
    def set_exposure_time(self, value: float) -> str: ...   # the feature written
    def gain(self) -> float: ...
    def set_gain(self, value: float) -> str: ...
    def gain_selectors(self) -> list[str]: ...
    def select_gain(self, selector: str) -> None: ...
    def acquisition_mode(self) -> str: ...
    def set_acquisition_mode(self, mode: str) -> None: ...
    def enable_trigger(self, selector: str, source: str) -> None: ...
    def disable_trigger(self, selector: str) -> None: ...
    def trigger_software(self) -> None: ...
    def roi(self) -> Roi: ...
    def set_roi(self, roi: Roi) -> None: ...
    def binning(self) -> tuple[int, int]: ...
    def set_binning(self, horizontal: int, vertical: int) -> None: ...
    def frame_rate(self) -> float: ...
    def set_frame_rate(self, hz: float) -> str: ...
    def disable_frame_rate(self) -> None: ...
    def user_sets(self) -> list[str]: ...
    def load_user_set(self, user_set: str) -> None: ...
    def save_user_set(self, user_set: str) -> None: ...
    def set_default_user_set(self, user_set: str) -> str: ...

    # Introspection.
    def nodes(self) -> list[str]: ...
    def node_info(self, name: str) -> Optional[NodeInfo]: ...
    def all_node_info(self) -> list[NodeInfo]: ...
    def categories(self) -> dict[str, list[str]]: ...
    def skipped(self) -> list[SkippedNode]: ...

    # Commands. execute: True once the camera reports completion; False with a
    # RuntimeWarning if it does not within `timeout` (the command was still
    # sent); False with wait=False.
    def execute(self, name: str, *, wait: bool = True, timeout: float = 5.0) -> bool: ...
    def is_done(self, name: str) -> bool: ...

    # The streamable features to a file and back. save: {"values": int,
    # "left_out": [(feature, reason)]}; load: {"written": int,
    # "written_on_retry": int, "failures": [str], "warnings": [str]}.
    def save_features(self, path: str | os.PathLike) -> dict: ...
    def load_features(self, path: str | os.PathLike) -> dict: ...

    def acquisition_start(self) -> None: ...
    def acquisition_stop(self) -> None: ...

    # Time.
    def time_calibrate(self, samples: int = 8, interval_ms: int = 20) -> None: ...  # GigE
    def time_reset(self) -> None: ...
    def map_dev_ts(self, ticks: int) -> Optional[float]: ...   # POSIX seconds

    # Chunks and events. A value that could not be read maps to its exception.
    def configure_chunks(self, selectors: Sequence[str], active: bool = True) -> None: ...
    def chunk_features(self, frame: Frame) -> dict[str, str | GenicamError]: ...
    def events(self, names: Sequence[str], iface: Optional[str] = None, port: int = 0) -> EventStream: ...  # GigE
    def event_features(self, event: Event) -> dict[str, str | GenicamError]: ...

    # Registers past GenApi; a write forgets every cached value.
    def read_register(self, address: int, length: int = 4) -> bytes: ...
    def write_register(self, address: int, data: bytes) -> None: ...

    # Control privilege and IP configuration (GigE).
    def reclaim_control(self) -> None: ...
    def release_control(self) -> None: ...
    def set_persistent_ip(self, ip: str, subnet: str, gateway: str = "0.0.0.0") -> None: ...

    def stream(
        self,
        iface: Optional[str] = None,
        packet_size: Optional[int] = None,
        auto_packet_size: bool = False,
        multicast: Optional[str] = None,
        destination_port: Optional[int] = None,
        packet_delay_ns: Optional[int] = None,
        resend: bool = True,
        deliver_incomplete: bool = False,
        monitor: bool = False,
    ) -> FrameStream: ...

class ControlState(str, Enum):
    HELD = "Held"; MONITOR = "Monitor"; LOST = "Lost"; RELEASED = "Released"

@dataclass(frozen=True)
class Roi:
    x: int
    y: int
    width: int
    height: int
```

## Features

```python
@dataclass(frozen=True)
class FeatureState:
    kind: str                          # a NodeKind value
    value: int | float | bool | str | None
    access: Optional[str]              # now: "RO" | "RW" | "WO" | None
    is_implemented: bool
    is_available: bool
    range: Optional[ValueRange]        # ValueRange(min, max, inc)
    enum_entries: Optional[list[str]]  # accepted now
    unit: Optional[str]
    polling_time: Optional[float]      # seconds
    caching: str                       # "NoCache" | "WriteThrough" | "WriteAround"

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
    def __enter__(self) -> "FrameStream": ...      # acquisition_start(), unless monitor=True
    def __exit__(self, *exc) -> None: ...           # acquisition_stop() + close()
    def __iter__(self) -> Iterator[Frame]: ...
    def __next__(self) -> Frame: ...                # 5-second default timeout
    def next_frame(self, timeout_ms: Optional[int] = None) -> Optional[Frame]: ...
    def next_block(self, timeout_ms: Optional[int] = None) -> Frame | EvsBlock | None: ...
    def stats(self) -> StreamStats: ...             # GigE
    def close(self) -> None: ...
```

`StreamStats` is a frozen dataclass with every counter of the receive engine —
`frames_complete`, `frames_incomplete`, `frames_missed`, `frames_lost`,
`packets_received`, `packets_missing`, `resend_requested`, `resend_recovered`,
`avg_fps`, `elapsed` and the rest; durations are in seconds.

## Frame

```python
class Frame:
    width: int
    height: int
    pixel_format: str
    pixel_format_code: int
    block_id: int
    payload_type: int                  # 0x0001 image, 0x4001 image with chunks
    padding_x: int                     # bytes after each line
    status: FrameStatus
    is_complete: bool
    missing_packets: Optional[int]     # for INCOMPLETE
    device_status: Optional[int]       # for DEVICE_STATUS
    chunks: Optional[dict[int, bytes]]
    ts_dev: Optional[int]
    ts_host: Optional[float]

    @classmethod
    def from_bytes(cls, payload: bytes, width: int, height: int,
                   pixel_format: str | int, padding_x: int = 0) -> "Frame": ...
    def payload(self) -> bytes: ...
    def to_numpy(self, demosaic: bool = False) -> NDArray[np.uint8] | NDArray[np.uint16]: ...
    def to_rgb8(self) -> NDArray[np.uint8]: ...     # always (H, W, 3)

class FrameStatus(str, Enum):
    COMPLETE = "Complete"; INCOMPLETE = "Incomplete"
    DEVICE_STATUS = "DeviceStatus"; TRAILER_MISSING = "TrailerMissing"
```

What shape and type `to_numpy` returns for each format:
[Streaming → to_numpy()](streaming.md#to_numpy--the-cameras-values).

## EvsBlock

```python
class EvsBlock:                        # from FrameStream.next_block()
    format: str                        # "evt3.0" | "evt2.1"
    block_id: int
    status: FrameStatus
    leader_size_x: int                 # GVSP leader and trailer, as sent
    leader_size_y: int
    trailer_size_y: int
    chunks: Optional[dict[int, bytes]]
    ts_dev: Optional[int]
    ts_host: Optional[float]

    def payload(self) -> bytes: ...    # the EVT bytes as sent
```

See [Streaming → Event-vision blocks](streaming.md#event-vision-blocks).

## Events

```python
class EventStream:                     # from Camera.events(); a context manager
    port: int
    def next_event(self, timeout_ms: Optional[int] = None) -> Event: ...
    def __iter__(self) -> Iterator[Event]: ...
    def close(self) -> None: ...

class Event:
    id: int
    stream_channel: int
    block_id: int
    ts_dev: int
    ts_host: Optional[float]
    data: bytes
```

## Network commands

```python
vg.send_action(device_key: int, group_key: int, group_mask: int,
               scheduled_time: Optional[int] = None,
               destination: str = "255.255.255.255",
               timeout_ms: int = 200) -> AckSummary     # AckSummary(sent, acks)
vg.force_ip(mac: str, ip: str, subnet: str, gateway: str = "0.0.0.0",
            iface: Optional[str] = None) -> None
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
        pixel_format: str = "Mono8",           # any PFNC name
        *,
        packet_resend: bool = True,
        drop_packets: Optional[Sequence[tuple[int, int]]] = None,  # (block, packet)
        delay_ack: Optional[Mapping[int, float]] = None,           # address -> seconds
    ) -> None: ...
    def start(self) -> None: ...
    def stop(self) -> None: ...
    ip: str
    port: int
    def device_info(self, timeout_ms: int = 1500) -> GigeDeviceInfo: ...
    def __enter__(self) -> "FakeGigeCamera": ...    # calls start()
    def __exit__(self, *exc) -> None: ...            # calls stop()

    # What the camera itself holds, for tests:
    def controller(self) -> Optional[str]: ...       # "ip:port" holding control
    def revoke_control(self) -> None: ...
    def peek(self, address: int, length: int) -> bytes: ...
    def software_triggers(self) -> int: ...
    def saved_exposure(self, user_set: int) -> Optional[float]: ...
    def actions(self) -> int: ...
    def blocks_sent(self) -> int: ...
    def writes(self) -> int: ...
    REGISTERS: Mapping[str, int]                     # its register map
    DEVICE_KEY: int; GROUP_KEY: int; GROUP_MASK: int; MAC: str
```

An in-process fake GigE Vision camera, for tests and demos without hardware.
Keep `port=3956`: discovery only looks there, so `device_info()` and
`vg.connect_gige` cannot find a fake bound elsewhere, and only one fake per
host can hold that port at a time.

```python
from viva_genicam.testing import FakeU3vCamera

class FakeU3vCamera:
    def __init__(self, width: int = 640, height: int = 480,
                 pixel_format: str = "Mono8") -> None: ...
    def connect(self) -> Camera: ...                 # as connect_u3v opens one
    def acquiring(self) -> bool: ...
    def peek(self, address: int, length: int) -> bytes: ...
```

An in-process fake USB3 Vision camera. No USB enumeration finds it, so
`connect()` opens it.

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
`except vg.GenicamError:` covers it. A panic in the Rust layer surfaces as
`pyo3_runtime.PanicException`, a `BaseException` — see
[Control & introspection](control.md#error-model).
