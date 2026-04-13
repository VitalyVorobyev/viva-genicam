"""FrameStream wrapper: sync iterator over live camera frames."""

from __future__ import annotations

from typing import Any, Iterator, Optional

from .frame import Frame


class FrameStream:
    """Context-manager + iterator that yields ``Frame`` objects.

    ``acquisition_start()`` is called on entry and ``acquisition_stop()``
    on exit, so ``with cam.stream() as frames:`` is the recommended
    pattern.
    """

    __slots__ = ("_native", "_camera", "_started")

    def __init__(self, native: Any, camera: Any) -> None:
        self._native = native
        self._camera = camera
        self._started = False

    def __enter__(self) -> "FrameStream":
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

    def close(self) -> None:
        self._native.close()


__all__ = ["FrameStream"]
