"""Frame wrapper around the native ``_Frame`` handle."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Optional

if TYPE_CHECKING:
    import numpy as np


class Frame:
    """One image frame produced by the camera.

    Wraps a native handle — the payload is only materialized into Python
    memory when ``payload()``, ``to_numpy()``, or ``to_rgb8()`` is called.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    @property
    def width(self) -> int:
        return self._native.width

    @property
    def height(self) -> int:
        return self._native.height

    @property
    def pixel_format(self) -> str:
        """Human-readable PFNC name, e.g. ``"Mono8"`` or ``"BayerRG8"``."""
        return self._native.pixel_format

    @property
    def pixel_format_code(self) -> int:
        """Raw PFNC numeric code."""
        return self._native.pixel_format_code

    @property
    def ts_dev(self) -> Optional[int]:
        """Device tick timestamp (camera-local), if reported."""
        return self._native.ts_dev

    @property
    def ts_host(self) -> Optional[float]:
        """Host-mapped timestamp as POSIX seconds, if available."""
        return self._native.ts_host

    def payload(self) -> bytes:
        """Raw payload as ``bytes`` (one copy)."""
        return self._native.payload()

    def to_numpy(self) -> "np.ndarray":
        """Return a NumPy array shaped for the pixel format.

        - Mono8 → ``(H, W) uint8``
        - Mono16 → ``(H, W) uint16``
        - RGB8 / BGR8 / Bayer8 → ``(H, W, 3) uint8``
        - unknown → ``(N,) uint8``
        """
        return self._native.to_numpy()

    def to_rgb8(self) -> "np.ndarray":
        """Always return an ``(H, W, 3) uint8`` RGB array."""
        return self._native.to_rgb8()

    def __repr__(self) -> str:
        return self._native.__repr__()


__all__ = ["Frame"]
