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

from . import _native
from .discovery import GigeDeviceInfo

_Native = _native.testing.FakeGigeCamera


class FakeGigeCamera:
    """An in-process GigE Vision camera useful for tests and demos.

    GenICam discovery is hard-wired to UDP port 3956; changing the
    ``port`` argument still lets the fake bind to a custom socket, but
    ``vg.connect_gige`` (and ``device_info()``) will not find it.
    Keep ``port=3956`` for the normal flow; only one fake camera per
    host can use that port at a time.
    """

    __slots__ = ("_native",)

    def __init__(
        self,
        width: int = 640,
        height: int = 480,
        fps: int = 30,
        bind_ip: str = "127.0.0.1",
        port: int = 3956,
        pixel_format: str = "Mono8",
    ) -> None:
        self._native = _Native(width, height, fps, bind_ip, port, pixel_format)

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

    def __enter__(self) -> "FakeGigeCamera":
        self._native.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self._native.stop()

    def __repr__(self) -> str:
        return repr(self._native)


__all__ = ["FakeGigeCamera"]
