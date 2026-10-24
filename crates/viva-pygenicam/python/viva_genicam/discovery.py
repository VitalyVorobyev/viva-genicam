"""Camera discovery: GigE Vision (network) and USB3 Vision (USB)."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, Optional, Union

from . import _native


@dataclass(frozen=True)
class GigeDeviceInfo:
    """A discovered GigE Vision camera."""

    ip: str
    mac: str
    manufacturer: Optional[str]
    model: Optional[str]
    transport: Literal["gige"] = "gige"
    _handle: Any = field(default=None, repr=False, compare=False)

    @classmethod
    def _from_native(cls, native: Any) -> "GigeDeviceInfo":
        return cls(
            ip=native.ip,
            mac=native.mac,
            manufacturer=native.manufacturer,
            model=native.model,
            _handle=native,
        )

    def to_dict(self) -> dict:
        return {
            "ip": self.ip,
            "mac": self.mac,
            "manufacturer": self.manufacturer,
            "model": self.model,
            "transport": self.transport,
        }


@dataclass(frozen=True)
class U3vDeviceInfo:
    """A discovered USB3 Vision camera.

    ``bus`` is the OS's identifier for the USB bus, as text; its form
    differs between operating systems. ``speed`` is the link speed the OS
    reports, such as ``"high speed (480 Mbit/s, USB 2)"``, or ``None`` when
    it reports none.
    """

    bus: str
    address: int
    vendor_id: int
    product_id: int
    serial: Optional[str]
    manufacturer: Optional[str]
    model: Optional[str]
    speed: Optional[str] = None
    transport: Literal["u3v"] = "u3v"
    _handle: Any = field(default=None, repr=False, compare=False)

    @classmethod
    def _from_native(cls, native: Any) -> "U3vDeviceInfo":
        return cls(
            bus=native.bus,
            address=native.address,
            vendor_id=native.vendor_id,
            product_id=native.product_id,
            serial=native.serial,
            manufacturer=native.manufacturer,
            model=native.model,
            speed=native.speed,
            _handle=native,
        )

    def to_dict(self) -> dict:
        return {
            "bus": self.bus,
            "address": self.address,
            "vendor_id": self.vendor_id,
            "product_id": self.product_id,
            "serial": self.serial,
            "manufacturer": self.manufacturer,
            "model": self.model,
            "speed": self.speed,
            "transport": self.transport,
        }


DeviceInfo = Union[GigeDeviceInfo, U3vDeviceInfo]


def discover(
    timeout_ms: int = 500,
    iface: Optional[str] = None,
    all: bool = False,
    allow_broadcast_ack: bool = False,
) -> list[GigeDeviceInfo]:
    """Discover GigE Vision cameras on the local network.

    Args:
        timeout_ms: How long to wait for responses.
        iface: If given, restrict discovery to one host interface, named
            either by one of its IPv4 addresses (``"169.254.105.106"``) or by
            its OS name (``"en0"``, or a GUID on Windows). ``viva-camctl
            --iface`` and ``viva-service --iface`` accept the same two
            spellings.
        all: If True, also search the loopback interface (where a local fake
            camera answers). Every routable non-loopback interface is
            searched either way.
        allow_broadcast_ack: If True, let a camera that cannot answer the
            host directly — one configured for another subnet, typically —
            broadcast its answer instead. Every listener on the subnet then
            receives it, which is why it is off by default.
            ``viva-camctl list --allow-broadcast-ack`` does the same.

    Returns:
        A list of ``GigeDeviceInfo`` — may be empty.
    """
    natives = _native.discover_gige(timeout_ms, iface, all, allow_broadcast_ack)
    return [GigeDeviceInfo._from_native(n) for n in natives]


def discover_ip(ip: str, timeout_ms: int = 500) -> Optional[GigeDeviceInfo]:
    """Ask the GigE Vision camera at ``ip`` directly (unicast discovery).

    For a camera that broadcast discovery does not reach, such as one on a
    routed subnet. ``viva-camctl list --ip`` does the same.

    Returns:
        The camera's ``GigeDeviceInfo``, or ``None`` when nothing answered
        within ``timeout_ms``.
    """
    native = _native.discover_gige_ip(ip, timeout_ms)
    return None if native is None else GigeDeviceInfo._from_native(native)


def discover_u3v() -> list[U3vDeviceInfo]:
    """Enumerate USB3 Vision cameras connected to the system."""
    natives = _native.discover_u3v()
    return [U3vDeviceInfo._from_native(n) for n in natives]


__all__ = [
    "GigeDeviceInfo",
    "U3vDeviceInfo",
    "DeviceInfo",
    "discover",
    "discover_ip",
    "discover_u3v",
]
