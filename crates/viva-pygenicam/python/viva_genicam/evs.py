"""EvsBlock wrapper around the native ``EvsBlock`` handle."""

from __future__ import annotations

from typing import Any, Optional

from .frame import FrameStatus


class EvsBlock:
    """One block of event data from an event-vision camera.

    An event camera, such as a LUCID Triton2 EVS, streams Prophesee EVT
    words instead of images. :meth:`FrameStream.next_block` hands such a
    block out as an ``EvsBlock``, with the encoded bytes exactly as the
    camera sent them. A block may be passed to and read from any thread.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    @property
    def format(self) -> str:
        """The EVT encoding: ``"evt3.0"`` (16-bit words) or ``"evt2.1"``
        (64-bit events)."""
        return self._native.format

    @property
    def ts_dev(self) -> Optional[int]:
        """Device tick timestamp from the GVSP leader, if reported."""
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
        """The block id the camera gave the block."""
        return self._native.block_id

    @property
    def status(self) -> FrameStatus:
        """How the block ended, as for a :class:`Frame`."""
        return FrameStatus(self._native.status)

    @property
    def leader_size_x(self) -> int:
        """Size X from the GVSP leader, as sent: not an image width."""
        return self._native.leader_size_x

    @property
    def leader_size_y(self) -> int:
        """Size Y from the GVSP leader, as sent: not an image height."""
        return self._native.leader_size_y

    @property
    def trailer_size_y(self) -> int:
        """Size Y from the GVSP trailer, as sent."""
        return self._native.trailer_size_y

    @property
    def chunks(self) -> Optional[dict[int, bytes]]:
        """The block's chunk data by chunk ID, or ``None`` for a block
        without chunks."""
        return self._native.chunks

    def payload(self) -> bytes:
        """The encoded EVT bytes as ``bytes`` (one copy)."""
        return self._native.payload()

    def __repr__(self) -> str:
        return self._native.__repr__()


__all__ = ["EvsBlock"]
