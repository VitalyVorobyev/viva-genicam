"""Frame wrapper around the native ``Frame`` handle."""

from __future__ import annotations

from enum import Enum
from typing import TYPE_CHECKING, Any, Optional, Union

from . import _native

if TYPE_CHECKING:
    import numpy as np
    from numpy.typing import NDArray


class FrameStatus(str, Enum):
    """How a frame's block ended, as ``viva_genicam::FrameStatus`` names it."""

    #: Every packet arrived and the camera reported success.
    COMPLETE = "Complete"
    #: The trailer arrived, packets did not: ``Frame.missing_packets`` of
    #: them, whose bytes are zero in the payload.
    INCOMPLETE = "Incomplete"
    #: The trailer reported an error status, ``Frame.device_status``.
    DEVICE_STATUS = "DeviceStatus"
    #: No trailer arrived, so how many packets the block had is unknown.
    TRAILER_MISSING = "TrailerMissing"


class Frame:
    """One image frame produced by the camera.

    Wraps a native handle — the payload is only materialized into Python
    memory when ``payload()``, ``to_numpy()``, or ``to_rgb8()`` is called.
    A frame may be passed to and read from any thread.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    @classmethod
    def from_bytes(
        cls,
        payload: bytes,
        width: int,
        height: int,
        pixel_format: Union[str, int],
        padding_x: int = 0,
    ) -> "Frame":
        """A frame from bytes received or saved elsewhere.

        ``payload`` holds the image's lines one after another, each followed
        by ``padding_x`` bytes; ``pixel_format`` is a PFNC name such as
        ``"Mono12p"`` or its numeric code.
        """
        return cls(_native.Frame(payload, width, height, pixel_format, padding_x))

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
        """The device timestamp mapped to host time, as POSIX seconds.

        ``None`` unless the camera was calibrated with
        :meth:`Camera.time_calibrate` before the stream opened.
        """
        return self._native.ts_host

    @property
    def block_id(self) -> int:
        """The block id the camera gave the frame."""
        return self._native.block_id

    @property
    def payload_type(self) -> int:
        """The GVSP payload type: ``0x0001`` an image, ``0x4001`` an image
        with chunks."""
        return self._native.payload_type

    @property
    def padding_x(self) -> int:
        """Bytes the camera sends after every line, which are not pixels."""
        return self._native.padding_x

    @property
    def status(self) -> FrameStatus:
        """How the frame's block ended. A stream hands out only complete
        frames unless it was opened with ``deliver_incomplete=True``."""
        return FrameStatus(self._native.status)

    @property
    def is_complete(self) -> bool:
        return self._native.is_complete

    @property
    def missing_packets(self) -> Optional[int]:
        """Packets that never arrived, for an ``INCOMPLETE`` frame."""
        return self._native.missing_packets

    @property
    def device_status(self) -> Optional[int]:
        """The error status the trailer reported, for a ``DEVICE_STATUS``
        frame."""
        return self._native.device_status

    @property
    def chunks(self) -> Optional[dict[int, bytes]]:
        """The frame's chunk data by chunk ID, or ``None`` for a frame without
        chunks. :meth:`Camera.chunk_features` reads the values they hold."""
        return self._native.chunks

    def payload(self) -> bytes:
        """Raw payload as ``bytes`` (one copy)."""
        return self._native.payload()

    def to_numpy(
        self, demosaic: bool = False
    ) -> "Union[NDArray[np.uint8], NDArray[np.uint16]]":
        """Return the image as a NumPy array, at the camera's own depth.

        - 8-bit monochrome → ``(H, W) uint8``
        - 10- to 16-bit monochrome, packed formats included → ``(H, W) uint16``
          holding the camera's values (a 12-bit pixel is ``0..4095``)
        - Bayer → the mosaic as sent, ``(H, W)``, ``uint8`` for 8-bit and
          ``uint16`` deeper; ``demosaic=True`` → ``(H, W, 3)`` RGB at the same
          depth (bilinear)
        - RGB / BGR (reordered to RGB) / Y'CbCr → ``(H, W, 3) uint8``
        - a format with no conversion → ``(N,) uint8``, the raw bytes

        Line padding the camera declares (``padding_x``) is skipped.
        ``demosaic`` has no effect on a format that is not Bayer.
        """
        return self._native.to_numpy(demosaic)

    def to_rgb8(self) -> "NDArray[np.uint8]":
        """Always return an ``(H, W, 3) uint8`` RGB array."""
        return self._native.to_rgb8()

    def __repr__(self) -> str:
        return self._native.__repr__()


__all__ = ["Frame", "FrameStatus"]
