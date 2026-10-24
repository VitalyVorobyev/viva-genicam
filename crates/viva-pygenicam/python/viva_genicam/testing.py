"""In-process fake cameras for testing and demos.

Lets you exercise the full bindings without any hardware. The fake
camera runs as background tasks inside the same process — no subprocess
spawn, no binary to ship.

Example:

    from viva_genicam.testing import FakeGigeCamera
    import viva_genicam as vg

    with FakeGigeCamera(width=640, height=480, fps=10) as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream() as frames:
            for frame in frames:
                print(frame.to_numpy().shape)
                break
"""

from __future__ import annotations

from typing import Mapping, Optional, Sequence

from . import _native
from .camera import Camera
from .discovery import GigeDeviceInfo

_Native = _native.FakeGigeCamera


class FakeGigeCamera:
    """An in-process GigE Vision camera useful for tests and demos.

    GenICam discovery is hard-wired to UDP port 3956; changing the
    ``port`` argument still lets the fake bind to a custom socket, but
    ``vg.connect_gige`` (and ``device_info()``) will not find it.
    Keep ``port=3956`` for the normal flow; only one fake camera per
    host can use that port at a time.

    ``pixel_format`` is any PFNC name (``"Mono8"``, ``"Mono12p"``,
    ``"RGB8"``, ...); the image is the fake's test pattern in that format.
    ``packet_resend=False`` makes a camera without packet resend.
    ``drop_packets`` lists ``(block_id, packet_id)`` pairs never sent — block
    ids count from 1 at each acquisition start. ``delay_ack`` maps a register
    address to the seconds the camera takes to acknowledge a command for it.

    The fake reports its own state — :meth:`controller`, :meth:`peek`,
    :meth:`software_triggers`, :meth:`actions` — so a test can check what
    reached the device, not what the library says it did.
    """

    __slots__ = ("_native",)

    #: The device key the fake answers action commands for.
    DEVICE_KEY: int = _Native.DEVICE_KEY
    #: The group key the fake answers action commands for.
    GROUP_KEY: int = _Native.GROUP_KEY
    #: The fake's group mask.
    GROUP_MASK: int = _Native.GROUP_MASK
    #: The fake's MAC address, as discovery reports it.
    MAC: str = _Native.MAC
    #: Register addresses, by the feature or bootstrap register they hold.
    REGISTERS: Mapping[str, int] = _Native.REGISTERS

    def __init__(
        self,
        width: int = 640,
        height: int = 480,
        fps: int = 30,
        bind_ip: str = "127.0.0.1",
        port: int = 3956,
        pixel_format: str = "Mono8",
        *,
        packet_resend: bool = True,
        drop_packets: Optional[Sequence[tuple[int, int]]] = None,
        delay_ack: Optional[Mapping[int, float]] = None,
    ) -> None:
        self._native = _Native(
            width,
            height,
            fps,
            bind_ip,
            port,
            pixel_format,
            packet_resend,
            list(drop_packets) if drop_packets is not None else None,
            dict(delay_ack) if delay_ack is not None else None,
        )

    def start(self) -> None:
        """Bind the sockets and spawn the GVCP/GVSP tasks."""
        self._native.start()

    def stop(self) -> None:
        """Abort the background tasks and release the sockets."""
        self._native.stop()

    @property
    def ip(self) -> str:
        return self._native.ip

    @property
    def port(self) -> int:
        return self._native.port

    def device_info(self, timeout_ms: int = 1500) -> GigeDeviceInfo:
        """Return a ``GigeDeviceInfo`` that resolves to this fake camera.

        Runs the normal GVCP discovery pipeline against loopback and
        returns the match, so the info is identical to what a user
        would get via ``vg.discover()``.
        """
        native = self._native.device_info(timeout_ms)
        return GigeDeviceInfo._from_native(native)

    def controller(self) -> Optional[str]:
        """The application holding control privilege, as ``"ip:port"``, or
        ``None`` when nobody holds it."""
        return self._native.controller()

    def revoke_control(self) -> None:
        """Take control privilege away on the camera's side, as a camera does
        when its controller's heartbeat expires."""
        self._native.revoke_control()

    def peek(self, address: int, length: int) -> bytes:
        """``length`` bytes of the camera's memory at ``address``, read past
        the protocol: what a write actually stored."""
        return self._native.peek(address, length)

    def software_triggers(self) -> int:
        """How many times ``TriggerSoftware`` was executed."""
        return self._native.software_triggers()

    def saved_exposure(self, user_set: int) -> Optional[float]:
        """The exposure ``UserSetSave`` stored in ``user_set`` (0 is
        ``Default``), or ``None`` if nothing was saved there."""
        return self._native.saved_exposure(user_set)

    def actions(self) -> int:
        """Action commands addressed to this camera that it acted on."""
        return self._native.actions()

    def blocks_sent(self) -> int:
        """GVSP blocks produced, those ``drop_packets`` damaged included."""
        return self._native.blocks_sent()

    def discovery_flags(self) -> list[int]:
        """The flag byte of every discovery command received, oldest first:
        ``0x01`` asks for an acknowledgement, ``0x10`` allows a broadcast
        one."""
        return self._native.discovery_flags()

    def writes(self) -> int:
        """Register writes received, from any application."""
        return self._native.writes()

    def __enter__(self) -> "FakeGigeCamera":
        self._native.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self._native.stop()

    def __repr__(self) -> str:
        return repr(self._native)


class FakeU3vCamera:
    """An in-process USB3 Vision camera, for tests and demos.

    Its registers, GenCP and stream protocol are laid out from the USB3 Vision
    standard. No USB enumeration finds it: :meth:`connect` opens it, as
    :func:`viva_genicam.connect_u3v` opens a camera on USB.

    ``pixel_format`` is a PFNC name, as for :class:`FakeGigeCamera`.
    :meth:`acquiring` and :meth:`peek` report the camera's own state, so a test
    can check what reached the device.
    """

    __slots__ = ("_native",)

    def __init__(
        self, width: int = 640, height: int = 480, pixel_format: str = "Mono8"
    ) -> None:
        self._native = _native.FakeU3vCamera(width, height, pixel_format)

    def connect(self) -> Camera:
        """Open the camera."""
        return Camera(self._native.connect())

    def acquiring(self) -> bool:
        """Whether ``AcquisitionStart`` reached the camera and
        ``AcquisitionStop`` has not."""
        return self._native.acquiring()

    def peek(self, address: int, length: int) -> bytes:
        """``length`` bytes of the camera's registers at ``address``."""
        return self._native.peek(address, length)

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = ["FakeGigeCamera", "FakeU3vCamera"]
