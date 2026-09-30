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

    def execute(self, name: str) -> None:
        """Execute a ``Command`` feature.

        ``node_info(name).kind == "Command"`` identifies one. Commands have no
        value: the camera acts on the write itself.

        >>> cam.set("UserSetSelector", "Default")
        >>> cam.execute("UserSetLoad")

        ``set(name, "1")`` does the same thing and always did, which is how
        this gap went unnoticed (issue #121) — but it reads like a write and
        the value it takes is discarded, so prefer ``execute``.

        Reads after the command re-read the camera for every feature whose
        register names the command's register in ``<pInvalidator>`` — FLIR's
        XML does this for ``ExposureTime`` after ``UserSetLoad``, for example.
        A feature with no such link keeps its cached value; reconnect to read
        it back.

        **``pIsDone`` polling is not implemented**, so this returns once the
        register write is acknowledged rather than once the camera has
        finished acting on it. A short sleep helps with this.
        """
        self._native.execute(name)

    def acquisition_start(self) -> None:
        self._native.acquisition_start()

    def acquisition_stop(self) -> None:
        self._native.acquisition_stop()

    # ── streaming ────────────────────────────────────────────────────────

    def stream(
        self,
        iface: Optional[str] = None,
        packet_size: Optional[int] = None,
        auto_packet_size: bool = False,
        multicast: Optional[str] = None,
        destination_port: Optional[int] = None,
    ) -> FrameStream:
        """Open a frame stream (context-manager yielding ``Frame`` objects).

        ``iface`` names the host interface that receives the stream, either by
        one of its IPv4 addresses (``"169.254.105.106"``) or by its OS name
        (``"en0"``, or a GUID on Windows); omit it to let the OS pick the
        interface that routes to the camera.

        Packet size: default leaves the camera's ``GevSCPSPacketSize`` alone;
        ``auto_packet_size=True`` sets it from the NIC MTU and path-probes;
        ``packet_size=N`` writes an explicit ceiling. Pass at most one of
        ``auto_packet_size`` and ``packet_size``. GigE-only parameters are
        ignored for U3V cameras.
        """
        native = self._native.open_stream(
            iface, packet_size, auto_packet_size, multicast, destination_port
        )
        return FrameStream(native, self)


def connect_gige(device_info: GigeDeviceInfo, iface: Optional[str] = None) -> Camera:
    """Connect to a GigE Vision camera.

    ``iface`` accepts either one of the host interface's IPv4 addresses or its
    OS name; omit it to let the OS pick the interface that routes to the
    camera.
    """
    native = _native.connect_gige(device_info._handle, iface)
    return Camera(native)


def connect_u3v(device_info: U3vDeviceInfo) -> Camera:
    """Connect to a USB3 Vision camera."""
    native = _native.connect_u3v(device_info._handle)
    return Camera(native)


__all__ = ["Camera", "connect_gige", "connect_u3v"]
