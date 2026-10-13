"""Camera facade: unified API over GigE and U3V transports."""

from __future__ import annotations

import os
import warnings
from dataclasses import dataclass
from enum import Enum
from typing import Any, Optional, Sequence, Union

from . import _native
from .discovery import DeviceInfo, GigeDeviceInfo, U3vDeviceInfo
from .errors import GenicamError
from .events import Event, EventStream
from .frame import Frame
from .node import FeatureState, NodeInfo, SkippedNode
from .stream import FrameStream


class ControlState(str, Enum):
    """Where a GigE Vision session stands with the camera's control
    privilege (``viva_genicam::ControlState``). Only ``HELD`` writes."""

    #: This application holds control and keeps it alive.
    HELD = "Held"
    #: Opened read-only (``monitor=True``): nothing claimed, nothing written.
    MONITOR = "Monitor"
    #: Control was lost — the heartbeat expired, or the camera gave it to
    #: another application. :meth:`Camera.reclaim_control` asks again.
    LOST = "Lost"
    #: Given up with :meth:`Camera.release_control`.
    RELEASED = "Released"


@dataclass(frozen=True)
class Roi:
    """A region of interest: the offsets of its top-left corner on the
    sensor, and its size, in pixels."""

    x: int
    y: int
    width: int
    height: int


class Camera:
    """Unified camera handle.

    Construct via ``connect_gige(device_info)`` or ``connect_u3v(device_info)``.
    The class itself presents the same method surface regardless of transport;
    a GigE-only method raises ``GenicamError`` on a USB3 Vision camera.

    Any thread may use a camera; calls from several threads are serialised.
    Every call that waits on the camera lets other Python threads run.

    ``with connect_gige(info) as cam:`` closes the camera at the end of the
    block, as :meth:`close` does: a GigE camera's control privilege is given
    back at once, rather than when the camera is garbage-collected.
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

    # ── lifetime ─────────────────────────────────────────────────────────

    def close(self) -> None:
        """Close the camera. A GigE camera gives control privilege back, so
        another application — or this one — can take it at once. Every later
        call raises ``GenicamError``; closing twice does nothing."""
        self._native.close()

    @property
    def closed(self) -> bool:
        """Whether :meth:`close` was called."""
        return self._native.closed

    def __enter__(self) -> "Camera":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

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

    def get_integer(self, name: str) -> int:
        """Read an ``Integer``, ``IntConverter`` or integer ``SwissKnife``;
        a ``Converter`` rounded to the nearest integer."""
        return self._native.get_integer(name)

    def set_integer(self, name: str, value: int) -> None:
        """Write an integer feature, within its bounds and increment."""
        self._native.set_integer(name, value)

    def get_float(self, name: str) -> float:
        """Read a ``Float``, ``Converter``, float ``SwissKnife`` or
        ``IntConverter``. An ``Integer`` is read with :meth:`get_integer`."""
        return self._native.get_float(name)

    def set_float(self, name: str, value: float) -> None:
        """Write a float feature, within its bounds."""
        self._native.set_float(name, value)

    def get_bool(self, name: str) -> bool:
        return self._native.get_bool(name)

    def set_bool(self, name: str, value: bool) -> None:
        self._native.set_bool(name, value)

    def get_enum(self, name: str) -> str:
        """The name of an enumeration's current entry."""
        return self._native.get_enum(name)

    def set_enum(self, name: str, entry: str) -> None:
        """Write an enumeration by entry name."""
        self._native.set_enum(name, entry)

    def get_string(self, name: str) -> str:
        """Read a string feature: a ``StringReg``, or a ``String`` holding a
        value or reading the node its ``pValue`` names."""
        return self._native.get_string(name)

    def set_string(self, name: str, value: str) -> None:
        """Write a string feature: a ``StringReg``, NUL-padded to its
        length, or a ``String`` (its value, or the node its ``pValue``
        names)."""
        self._native.set_string(name, value)

    def get_register(self, name: str) -> bytes:
        """Read a ``Register`` feature's bytes."""
        return self._native.get_register(name)

    def set_register(self, name: str, data: bytes) -> None:
        """Write a ``Register`` feature's bytes: exactly its length."""
        self._native.set_register(name, data)

    def feature_state(self, name: str) -> FeatureState:
        """Everything about a feature at once: its typed value, what the
        camera permits now, whether it is implemented and available, its
        range, the entries it accepts, its unit, and how to refresh it."""
        return FeatureState.from_dict(self._native.feature_state(name))

    def enum_entries(self, name: str) -> list[str]:
        """Every entry an enumeration declares, in the XML's order."""
        return self._native.enum_entries(name)

    def available_enum_entries(self, name: str) -> list[str]:
        """The entries an enumeration accepts now: those whose own
        ``pIsImplemented`` and ``pIsAvailable`` allow them."""
        return self._native.available_enum_entries(name)

    # ── what changed ─────────────────────────────────────────────────────

    def generation(self) -> int:
        """A counter every write and invalidation advances: where a viewer
        starts :meth:`changed_since` from."""
        return self._native.generation()

    def changed_since(self, since: int) -> tuple[list[str], int]:
        """The features changed after generation ``since`` — written, or
        invalidated — in document order, and the generation now."""
        names, now = self._native.changed_since(since)
        return names, now

    def poll(self) -> list[str]:
        """Invalidate the values whose ``PollingTime`` has passed since they
        were read, and return what to read again. Call it about as often as
        the shortest polling time you show."""
        return self._native.poll()

    def invalidate(self, name: str) -> None:
        """Forget the cached value of ``name`` — and of the registers it is
        read through and of everything that depends on it — so the next read
        asks the camera.

        For a value the camera changed in a way its XML does not describe. A
        feature the XML declares ``NoCache`` is read from the camera every
        time anyway, and one with a ``PollingTime`` once that has passed.
        """
        self._native.invalidate(name)

    def invalidate_all(self) -> None:
        """Forget every cached value, after something that may have changed
        the camera wholesale."""
        self._native.invalidate_all()

    # ── standard features ────────────────────────────────────────────────
    #
    # Each finds its feature by the names cameras declare it under — SFNC's,
    # and the older ones (`ExposureTimeAbs`, `GainRaw`, ...) — and raises
    # `GenicamError` naming them all when the camera has none.

    def exposure_time(self) -> float:
        """The exposure time, in the unit of the feature that holds it:
        microseconds for ``ExposureTime``/``ExposureTimeAbs``, device units
        for ``ExposureTimeRaw``."""
        return self._native.exposure_time()

    def set_exposure_time(self, value: float) -> str:
        """Write the exposure time through whichever feature the camera has:
        ``ExposureTime`` or ``ExposureTimeAbs`` (microseconds), else
        ``ExposureTimeRaw`` (device units). Returns that feature's name."""
        return self._native.set_exposure_time(value)

    def gain(self) -> float:
        """The gain of the selected gain control: dB for ``Gain``, device
        units for ``GainRaw``."""
        return self._native.gain()

    def set_gain(self, value: float) -> str:
        """Write the gain of the selected gain control through whichever
        feature the camera has: ``Gain`` (dB), else ``GainRaw`` (device
        units). Returns that feature's name."""
        return self._native.set_gain(value)

    def gain_selectors(self) -> list[str]:
        """The gain controls ``GainSelector`` offers, such as ``"All"``."""
        return self._native.gain_selectors()

    def select_gain(self, selector: str) -> None:
        """Select the gain control :meth:`gain` and :meth:`set_gain` act on."""
        self._native.select_gain(selector)

    def acquisition_mode(self) -> str:
        """``"Continuous"``, ``"SingleFrame"``, ``"MultiFrame"``, or a
        vendor's own."""
        return self._native.acquisition_mode()

    def set_acquisition_mode(self, mode: str) -> None:
        self._native.set_acquisition_mode(mode)

    def enable_trigger(self, selector: str, source: str) -> None:
        """Make trigger ``selector`` wait for ``source`` (``"Software"``,
        ``"Line1"``, ...): selected, given its source, then turned on. It
        stays selected for :meth:`trigger_software`."""
        self._native.enable_trigger(selector, source)

    def disable_trigger(self, selector: str) -> None:
        """Turn trigger ``selector`` off: the camera runs free again."""
        self._native.disable_trigger(selector)

    def trigger_software(self) -> None:
        """Fire the selected trigger once, if its source is ``"Software"``."""
        self._native.trigger_software()

    def roi(self) -> Roi:
        """The region of interest. A camera without offsets reads as one at
        the origin."""
        return Roi(*self._native.roi())

    def set_roi(self, roi: Roi) -> None:
        """Move and size the region of interest. Every value is checked
        against its feature's increment before anything is written, and the
        writes go in the order that lets any valid region be reached."""
        self._native.set_roi(roi.x, roi.y, roi.width, roi.height)

    def binning(self) -> tuple[int, int]:
        """The binning, ``(horizontal, vertical)``."""
        return self._native.binning()

    def set_binning(self, horizontal: int, vertical: int) -> None:
        """Combine ``horizontal`` × ``vertical`` pixels into one."""
        self._native.set_binning(horizontal, vertical)

    def frame_rate(self) -> float:
        """The acquisition frame rate, in frames per second."""
        return self._native.frame_rate()

    def set_frame_rate(self, hz: float) -> str:
        """Fix the frame rate at ``hz``, switching the camera's frame-rate
        enable on first where it has one. Returns the feature's name."""
        return self._native.set_frame_rate(hz)

    def disable_frame_rate(self) -> None:
        """Let the camera choose its frame rate again."""
        self._native.disable_frame_rate()

    def user_sets(self) -> list[str]:
        """The user sets ``UserSetSelector`` offers, ``"Default"`` among them."""
        return self._native.user_sets()

    def load_user_set(self, user_set: str) -> None:
        """Load ``user_set`` into the camera's settings and wait for it.
        Every cached value is forgotten afterwards."""
        self._native.load_user_set(user_set)

    def save_user_set(self, user_set: str) -> None:
        """Save the current settings into ``user_set`` and wait for it. A
        camera refuses to overwrite ``"Default"``."""
        self._native.save_user_set(user_set)

    def set_default_user_set(self, user_set: str) -> str:
        """Make ``user_set`` the one the camera loads at power-up. Returns the
        feature that holds it: ``UserSetDefault`` or ``UserSetDefaultSelector``."""
        return self._native.set_default_user_set(user_set)

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

    def skipped(self) -> list[SkippedNode]:
        """Declarations in the camera's XML that could not become features.

        Empty for a document the library fully understands. A feature you
        expected but cannot find may be here rather than absent from the
        camera — and worth an issue with the camera's XML attached.
        """
        return [SkippedNode.from_dict(d) for d in self._native.skipped()]

    # ── control ──────────────────────────────────────────────────────────

    def execute(self, name: str, *, wait: bool = True, timeout: float = 5.0) -> bool:
        """Execute a ``Command`` feature and wait until the camera has finished.

        ``node_info(name).kind == "Command"`` identifies one. Commands have no
        value: the camera acts on the write itself.

        >>> cam.set("UserSetSelector", "Default")
        >>> cam.execute("UserSetLoad")

        GenICam asks applications to wait for a command to complete before
        relying on its effect, by polling its ``IsDone``. ``execute`` does that
        for up to ``timeout`` seconds and returns ``True`` once the camera
        reports the command finished. If it has not by then, ``execute`` warns
        (``RuntimeWarning``) and returns ``False``: the command was still sent.
        Most commands write a write-only register and are done as soon as the
        write is acknowledged; a self-clearing one is waited on.
        ``wait=False`` returns ``False`` once the write is acknowledged,
        without asking.

        A command is executed, never set: ``set(name, "1")`` raises.

        Reads after the command re-read the camera for every feature whose
        register names the command's register in ``<pInvalidator>`` — FLIR's
        XML does this for ``ExposureTime`` after ``UserSetLoad``, for example.
        A feature with no such link keeps its cached value; reconnect to read
        it back.
        """
        done = self._native.execute(name, wait, timeout)
        if wait and not done:
            warnings.warn(
                f"{name} was executed, but the camera did not report completion "
                f"within {timeout} s",
                RuntimeWarning,
                stacklevel=2,
            )
        return done

    def is_done(self, name: str) -> bool:
        """Whether the command ``name`` has finished: GenICam's ``IsDone``."""
        return self._native.is_done(name)

    def save_features(self, path: Union[str, os.PathLike]) -> dict:
        """Save the camera's configuration — every feature its XML marks
        streamable — to ``path``.

        A feature behind a selector is saved under each of the selector's
        entries; behind one that cannot be set now, once. Returns ``{"values": int, "left_out": [(feature, reason)]}``;
        the file says the same in comments. Save while the camera is not
        acquiring: what acquisition locks is left out otherwise.
        """
        return self._native.save_features(os.fspath(path))

    def load_features(self, path: Union[str, os.PathLike]) -> dict:
        """Restore a configuration saved with :meth:`save_features`.

        Values are written in the order the camera's dependencies need, a
        failed write is tried once more after the others, and nothing stops
        the load early. Returns ``{"written": int, "written_on_retry": int,
        "failures": [str], "warnings": [str]}`` — a warning when the file was
        saved from another camera model. Raises only for a file that cannot
        be read.
        """
        return self._native.load_features(os.fspath(path))

    def acquisition_start(self) -> None:
        """Lock the transport parameters (``TLParamsLocked``) and execute
        ``AcquisitionStart``."""
        self._native.acquisition_start()

    def acquisition_stop(self) -> None:
        """Execute ``AcquisitionStop`` and unlock the transport parameters."""
        self._native.acquisition_stop()

    # ── time ─────────────────────────────────────────────────────────────

    def time_calibrate(self, samples: int = 8, interval_ms: int = 20) -> None:
        """Fit the camera-to-host clock mapping (GigE Vision only).

        Latches the camera's timestamp ``samples`` times, ``interval_ms``
        apart, and fits device ticks to host time. A stream opened afterwards
        sets each frame's ``ts_host``; without calibration ``ts_host`` is
        ``None``.

        >>> cam.time_calibrate()
        >>> with cam.stream() as frames:
        ...     frame = next(iter(frames))
        ...     frame.ts_host  # POSIX seconds
        """
        self._native.time_calibrate(samples, interval_ms)

    def time_reset(self) -> None:
        """Reset the camera's timestamp counter, where it has a command for
        it, and forget the clock fit."""
        self._native.time_reset()

    def map_dev_ts(self, ticks: int) -> Optional[float]:
        """Device timestamp ticks as host POSIX seconds, through the fit
        :meth:`time_calibrate` made; ``None`` before one."""
        return self._native.map_dev_ts(ticks)

    # ── chunks and events ────────────────────────────────────────────────

    def configure_chunks(self, selectors: Sequence[str], active: bool = True) -> None:
        """Turn chunk mode on (or off with ``active=False``) and enable the
        chunks named, such as ``["Timestamp", "ExposureTime"]``."""
        self._native.configure_chunks(list(selectors), active)

    def chunk_features(self, frame: Frame) -> dict[str, Union[str, GenicamError]]:
        """Every feature ``frame``'s chunks feed, read through the camera's
        XML: the feature's name to its value as text, or to the exception
        that kept it from being read. Empty for a frame without chunks."""
        return self._native.chunk_features(frame._native)

    def events(
        self,
        names: Sequence[str],
        iface: Optional[str] = None,
        port: int = 0,
    ) -> EventStream:
        """Receive the events ``names`` (``["ExposureEnd"]``).

        GigE Vision: binds a UDP socket on ``iface`` (``port``, 0 for any),
        points the camera's message channel at it, and enables each event;
        ``iface`` as for :meth:`stream`. USB3 Vision: enables each event and
        reads the camera's event interface; ``iface`` and ``port`` do not
        apply.
        """
        return EventStream(self._native.open_event_stream(list(names), iface, port))

    def event_features(self, event: Event) -> dict[str, Union[str, GenicamError]]:
        """Every feature the camera's XML derives from ``event`` — such as
        ``EventExposureEndTimestamp`` — to its value as text, or to the
        exception that kept it from being read."""
        return self._native.event_features(event._native)

    # ── registers ────────────────────────────────────────────────────────

    def read_register(self, address: int, length: int = 4) -> bytes:
        """``length`` bytes of the camera's register space at ``address``,
        past GenApi: a bootstrap register, or one the XML does not name."""
        return self._native.read_register(address, length)

    def write_register(self, address: int, data: bytes) -> None:
        """Write ``data`` at ``address``, past GenApi. Every cached feature
        value is forgotten, since the write may have changed any of them."""
        self._native.write_register(address, data)

    # ── control privilege (GigE Vision) ──────────────────────────────────

    @property
    def control_state(self) -> ControlState:
        """Where this session stands with control privilege. Losing it — the
        camera timed the heartbeat out, or gave control to another
        application — shows here as ``LOST``."""
        return ControlState(self._native.control_state)

    def reclaim_control(self) -> None:
        """Ask for control privilege again — after it was lost or released,
        or to turn a monitor into a controller — and keep it alive. Raises,
        and changes nothing, while another application holds it."""
        self._native.reclaim_control()

    def release_control(self) -> None:
        """Give control privilege up now, so another application can take it.
        :meth:`reclaim_control` and :meth:`stream` take it again."""
        self._native.release_control()

    def set_persistent_ip(self, ip: str, subnet: str, gateway: str = "0.0.0.0") -> None:
        """Store an IP configuration the camera uses from its next power-up,
        and switch its persistent-IP mode on. Needs control privilege."""
        self._native.set_persistent_ip(ip, subnet, gateway)

    # ── streaming ────────────────────────────────────────────────────────

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
    ) -> FrameStream:
        """Open a frame stream (context-manager yielding ``Frame`` objects).

        ``iface`` names the host interface that receives the stream, either by
        one of its IPv4 addresses (``"169.254.105.106"``) or by its OS name
        (``"en0"``, or a GUID on Windows); omit it to let the OS pick the
        interface that routes to the camera.

        Packet size: default leaves the camera's ``GevSCPSPacketSize`` alone;
        ``auto_packet_size=True`` sets it from the NIC MTU and path-probes;
        ``packet_size=N`` writes an explicit ceiling. Pass at most one of
        ``auto_packet_size`` and ``packet_size``.

        ``destination_port`` receives on a fixed UDP port, for a firewall that
        admits only it; by default the OS picks a free one.
        ``packet_delay_ns`` has the camera wait that long between packets
        (``GevSCPD``, converted to its timestamp ticks); by default the camera's
        own value is left alone. ``resend=False`` never asks the camera to send
        lost packets again; by default it is asked when it supports resend.

        ``deliver_incomplete=True`` also hands out frames that lost packets,
        their ``status`` saying how; by default only complete frames come out.

        ``monitor=True`` watches the multicast stream another application set
        up, claiming and writing nothing — the only stream a camera opened
        with ``connect_gige(..., monitor=True)`` can open. The stream then
        starts and stops no acquisition; the controlling application does.
        A USB3 Vision camera has no monitor access — one application opens
        the device — so ``monitor=True`` raises ``GenicamError`` on one.

        The other GigE-only parameters are ignored for U3V cameras.
        """
        native = self._native.open_stream(
            iface,
            packet_size,
            auto_packet_size,
            multicast,
            destination_port,
            packet_delay_ns,
            resend,
            deliver_incomplete,
            monitor,
        )
        return FrameStream(native, self, controls=not monitor)


def connect_gige(
    device_info: GigeDeviceInfo, iface: Optional[str] = None, monitor: bool = False
) -> Camera:
    """Connect to a GigE Vision camera.

    ``iface`` accepts either one of the host interface's IPv4 addresses or its
    OS name. Omit it to use the interface the camera was discovered on, or
    failing that the host interface whose subnet contains the camera's IP.

    The camera's control privilege is claimed, kept alive, and released when
    the camera is closed (or garbage-collected); opening fails while another
    application holds it. ``monitor=True`` opens read-only instead: nothing is
    claimed or written, so it works alongside that application, and every
    ``set`` fails.
    """
    native = _native.connect_gige(device_info._handle, iface, monitor)
    return Camera(native)


def connect_u3v(device_info: U3vDeviceInfo) -> Camera:
    """Connect to a USB3 Vision camera."""
    native = _native.connect_u3v(device_info._handle)
    return Camera(native)


__all__ = ["Camera", "ControlState", "Roi", "connect_gige", "connect_u3v"]
