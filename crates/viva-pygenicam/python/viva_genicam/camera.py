"""Camera facade: unified API over GigE and U3V transports."""

from __future__ import annotations

from typing import Any, Optional

from . import _native
from .discovery import DeviceInfo, GigeDeviceInfo, U3vDeviceInfo
from .node import NodeInfo
from .stream import FrameStream


class Camera:
    """Unified camera handle.

    Construct via ``connect_gige(device_info)`` or ``connect_u3v(device_info)``.
    The class itself presents the same method surface regardless of transport.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    # ── class factories ──────────────────────────────────────────────────

    @classmethod
    def open(cls, device: DeviceInfo, **kwargs) -> "Camera":
        """Connect to ``device`` using the appropriate transport."""
        if isinstance(device, GigeDeviceInfo):
            return connect_gige(device, **kwargs)
        if isinstance(device, U3vDeviceInfo):
            return connect_u3v(device)
        raise TypeError(f"unsupported device type: {type(device).__name__}")

    # ── metadata ─────────────────────────────────────────────────────────

    @property
    def transport(self) -> str:
        """``"gige"`` or ``"u3v"``."""
        return self._native.transport

    @property
    def xml(self) -> str:
        """Raw GenICam XML fetched from the camera."""
        return self._native.xml

    # ── feature access ───────────────────────────────────────────────────

    def get(self, name: str) -> str:
        """Read a feature value, formatted as a string."""
        return self._native.get(name)

    def set(self, name: str, value: str) -> None:
        """Set a feature value; the string is parsed per node type."""
        self._native.set(name, value)

    def set_exposure_time_us(self, value: float) -> None:
        self._native.set_exposure_time_us(value)

    def set_gain_db(self, value: float) -> None:
        self._native.set_gain_db(value)

    def enum_entries(self, name: str) -> list[str]:
        """List allowed entries for an enumeration feature."""
        return self._native.enum_entries(name)

    # ── introspection ────────────────────────────────────────────────────

    def nodes(self) -> list[str]:
        """Names of every feature exposed by the camera."""
        return self._native.nodes()

    def node_info(self, name: str) -> Optional[NodeInfo]:
        """Metadata for one feature, or ``None`` if not present."""
        raw = self._native.node_info(name)
        return NodeInfo.from_dict(raw) if raw is not None else None

    def all_node_info(self) -> list[NodeInfo]:
        """Metadata for every feature."""
        return [NodeInfo.from_dict(d) for d in self._native.all_node_info()]

    def categories(self) -> dict[str, list[str]]:
        """Category → feature-name children."""
        return dict(self._native.categories())

    # ── control ──────────────────────────────────────────────────────────

    def acquisition_start(self) -> None:
        self._native.acquisition_start()

    def acquisition_stop(self) -> None:
        self._native.acquisition_stop()

    # ── streaming ────────────────────────────────────────────────────────

    def stream(
        self,
        iface: Optional[str] = None,
        auto_packet_size: Optional[bool] = None,
        multicast: Optional[str] = None,
        destination_port: Optional[int] = None,
    ) -> FrameStream:
        """Open a frame stream (context-manager yielding ``Frame`` objects).

        GigE-only parameters are ignored for U3V cameras.
        """
        native = self._native.open_stream(
            iface, auto_packet_size, multicast, destination_port
        )
        return FrameStream(native, self)


def connect_gige(device_info: GigeDeviceInfo, iface: Optional[str] = None) -> Camera:
    """Connect to a GigE Vision camera."""
    native = _native.connect_gige(device_info._handle, iface)
    return Camera(native)


def connect_u3v(device_info: U3vDeviceInfo) -> Camera:
    """Connect to a USB3 Vision camera."""
    native = _native.connect_u3v(device_info._handle)
    return Camera(native)


__all__ = ["Camera", "connect_gige", "connect_u3v"]
