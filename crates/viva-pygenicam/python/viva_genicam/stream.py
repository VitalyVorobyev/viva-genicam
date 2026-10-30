"""FrameStream wrapper: sync iterator over live camera frames."""

from __future__ import annotations

from dataclasses import dataclass, fields
from typing import Any, Iterator, Optional, Union

from . import _native
from .evs import EvsBlock
from .frame import Frame


@dataclass(frozen=True)
class StreamStats:
    """A stream's counters (``viva_genicam::StreamStats``).

    Every block the camera sends is counted once, when its fate is decided::

        blocks sent = frames_complete + frames_incomplete + frames_missed
                    + frames_backpressure + frames_unsupported

    once nothing is in flight and the frames taken. Durations are seconds.
    A USB3 Vision stream counts bulk transfers in ``packets_received``, and
    the failed transfers it recovered from in ``transfer_errors``; its other
    packet and resend counters stay 0.
    """

    frames_complete: int
    frames_incomplete: int
    frames_missed: int
    frames_backpressure: int
    frames_unsupported: int
    packets_received: int
    packets_duplicate: int
    packets_missing: int
    packets_ignored_malformed: int
    packets_ignored_unsupported: int
    packets_ignored_out_of_range: int
    packets_ignored_late: int
    packets_ignored_error_status: int
    packets_ignored_foreign: int
    packets_size_mismatch: int
    resend_requested: int
    resend_recovered: int
    resend_unavailable: int
    #: USB3 Vision: failed transfers recovered from by resetting the endpoint.
    transfer_errors: int
    bytes: int
    #: Frames not received complete: incomplete, missed and dropped to
    #: backpressure.
    frames_lost: int
    #: Packets received and not used, for any reason.
    packets_ignored: int
    last_frame_dt: float
    avg_fps: float
    avg_mbps: float
    avg_latency_ms: Optional[float]
    elapsed: float
    packets_per_second: float

    @classmethod
    def from_dict(cls, d: dict) -> "StreamStats":
        return cls(**{f.name: d[f.name] for f in fields(cls)})


class FrameStream:
    """Context-manager + iterator that yields ``Frame`` objects.

    ``acquisition_start()`` is called on entry and ``acquisition_stop()``
    on exit, so ``with cam.stream() as frames:`` is the recommended
    pattern. A monitor's stream starts and stops nothing: the application
    that controls the camera does.
    """

    __slots__ = ("_native", "_camera", "_started", "_controls")

    def __init__(self, native: Any, camera: Any, controls: bool = True) -> None:
        self._native = native
        self._camera = camera
        self._started = False
        self._controls = controls

    def __enter__(self) -> "FrameStream":
        if self._controls:
            self._camera._native.acquisition_start()
            self._started = True
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        try:
            if self._started:
                self._camera._native.acquisition_stop()
        finally:
            self._started = False
            self._native.close()

    def __iter__(self) -> Iterator[Frame]:
        return self

    def __next__(self) -> Frame:
        return Frame(self._native.__next__())

    def next_frame(self, timeout_ms: Optional[int] = None) -> Optional[Frame]:
        """Pull the next frame with an optional per-call timeout in ms.

        Returns ``None`` on a clean stream end; raises on error or timeout.
        """
        native = self._native.next_frame(timeout_ms)
        return None if native is None else Frame(native)

    def next_block(self, timeout_ms: Optional[int] = None) -> Union[Frame, EvsBlock, None]:
        """Pull the next block: a :class:`Frame` for an image, an
        :class:`EvsBlock` for an event camera's data.

        :meth:`next_frame` hands out event data as a frame of raw bytes;
        this tells the two apart. A USB3 Vision stream returns frames.
        Returns ``None`` on a clean stream end; raises on error or timeout.
        """
        native = self._native.next_block(timeout_ms)
        if native is None:
            return None
        if isinstance(native, _native.EvsBlock):
            return EvsBlock(native)
        return Frame(native)

    def stats(self) -> StreamStats:
        """The stream's counters so far."""
        return StreamStats.from_dict(self._native.stats())

    def close(self) -> None:
        self._native.close()


__all__ = ["FrameStream", "StreamStats"]
