# Discovery

GigE and USB3 Vision have separate discovery pipelines. Both return frozen dataclasses you can pass straight to `connect_gige` / `connect_u3v`.

## GigE Vision

```python
import viva_genicam as vg

cams = vg.discover(timeout_ms=500)
for c in cams:
    print(c.ip, c.mac, c.model, c.manufacturer)
```

`vg.discover()` sends a GVCP `DISCOVERY_CMD` broadcast on the default outbound interface and collects ack packets for `timeout_ms` milliseconds. Returns a list of `GigeDeviceInfo`:

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
```

Use this when the host has multiple NICs and you only want to broadcast out one of them.

### Scan every NIC

```python
cams = vg.discover(timeout_ms=500, all=True)
```

Enumerates every local interface, broadcasts on each, and merges the results. This is what you want on a developer machine where you may not know ahead of time which NIC the camera is on.

### Slower cameras

Some cameras are slow to reply or sit on busy networks. Bump the timeout:

```python
cams = vg.discover(timeout_ms=3000, all=True)
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
```

When omitted, the stream interface is auto-resolved by matching the camera IP against every local NIC's subnet. For loopback (e.g. the fake camera) this resolves to `lo`/`lo0` automatically.

## Next

→ [Control & introspection](control.md) — read and write features, walk the NodeMap.
