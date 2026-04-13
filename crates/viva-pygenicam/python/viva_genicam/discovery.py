"""Camera discovery: GigE Vision (network) and USB3 Vision (libusb)."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Literal, Optional

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
    """A discovered USB3 Vision camera."""

    bus: int
    address: int
    vendor_id: int
    product_id: int
    serial: Optional[str]
    manufacturer: Optional[str]
    model: Optional[str]
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
            "transport": self.transport,
        }


DeviceInfo = GigeDeviceInfo | U3vDeviceInfo


def discover(
    timeout_ms: int = 500,
    iface: Optional[str] = None,
    all: bool = False,
) -> list[GigeDeviceInfo]:
    """Discover GigE Vision cameras on the local network.

    Args:
        timeout_ms: How long to wait for responses.
        iface: If given, restrict discovery to the named NIC (e.g. ``"en0"``).
        all: If True, enumerate all system interfaces and merge results.

    Returns:
        A list of ``GigeDeviceInfo`` — may be empty.
    """
    natives = _native.discover_gige(timeout_ms, iface, all)
    return [GigeDeviceInfo._from_native(n) for n in natives]


def discover_u3v() -> list[U3vDeviceInfo]:
    """Enumerate USB3 Vision cameras connected to the system."""
    natives = _native.discover_u3v()
    return [U3vDeviceInfo._from_native(n) for n in natives]


__all__ = ["GigeDeviceInfo", "U3vDeviceInfo", "DeviceInfo", "discover", "discover_u3v"]
