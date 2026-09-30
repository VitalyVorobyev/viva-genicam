# Discovery

GigE and USB3 Vision have separate discovery pipelines. Both return frozen dataclasses you can pass straight to `connect_gige` / `connect_u3v`.

## GigE Vision

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
for c in cams:
    print(c.ip, c.mac, c.model, c.manufacturer)
```

`vg.discover()` sends a GVCP discovery broadcast on every IPv4 interface except loopback and collects the replies for `timeout_ms` milliseconds. Returns a list of `GigeDeviceInfo`:

```python
@dataclass(frozen=True)
class GigeDeviceInfo:
    ip: str                        # "192.168.1.42"
    mac: str                       # "DE:AD:BE:EF:CA:FE"
    manufacturer: Optional[str]
    model: Optional[str]
    transport: Literal["gige"]
```

### Restrict to one NIC

```python
cams = vg.discover(timeout_ms=500, iface="en0")
cams = vg.discover(timeout_ms=500, iface="192.168.0.5")   # same thing
```

Use this when the host has multiple NICs and you only want to broadcast out one of them.

`iface=` accepts the host NIC's OS name or one of its IPv4 addresses. On Windows the name is a GUID like `{6394C55F-F630-4BC7-92D2-7AC320C73D1C}`, so the address is usually the easier value to supply.

### Include loopback

```python
cams = vg.discover(timeout_ms=500, all=True)
```

`all=True` adds the loopback interface to the scan. That is only useful for a
fake camera on `127.0.0.1` (see [Install & hello-camera](install.md#hello-camera--no-hardware-needed));
real cameras are already covered by the default scan. `iface=` takes precedence
when both are given.

### Slower cameras

Some cameras are slow to reply or sit on busy networks. Bump the timeout:

```python
cams = vg.discover(timeout_ms=3000)
```

## USB3 Vision

```python
cams = vg.discover_u3v()
for c in cams:
    print(f"vid:pid=0x{c.vendor_id:04x}:0x{c.product_id:04x}")
    print(f"  bus={c.bus} addr={c.address}")
    print(f"  model={c.model}  serial={c.serial}")
```

`vg.discover_u3v()` enumerates USB devices whose interface descriptors match the USB3 Vision class/subclass/protocol triple. Returns a list of `U3vDeviceInfo`:

```python
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

USB discovery is synchronous (there is no `timeout_ms` knob) and does not require any broadcast.

## Connecting

Either `DeviceInfo` type can be passed directly:

```python
cam = vg.connect_gige(cams[0])                 # GigE
cam = vg.connect_u3v(u3v_cams[0])              # U3V
cam = vg.Camera.open(cams[0])                  # dispatches on info type
```

`connect_gige` accepts an optional `iface=` override if you know which NIC should stream from the camera:

```python
cam = vg.connect_gige(info, iface="en0")
cam = vg.connect_gige(info, iface="192.168.0.5")   # same thing
```

When omitted, the stream interface is auto-resolved by matching the camera IP against every local NIC's subnet. For loopback (e.g. the fake camera) this resolves to `lo`/`lo0` automatically.

## Next

→ [Control & introspection](control.md) — read and write features, walk the NodeMap.
