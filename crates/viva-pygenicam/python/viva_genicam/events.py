"""GigE Vision events: what the camera reports on its message channel."""

from __future__ import annotations

from typing import Any, Iterator, Optional


class Event:
    """One event the camera sent, such as the end of an exposure.

    :meth:`Camera.event_features` reads the features the camera's XML
    derives from it, ``EventExposureEndTimestamp`` and the like.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    @property
    def id(self) -> int:
        """The event identifier: ``0x0005`` for the standard start of
        transfer, ``0x9000`` and above for a camera's own events."""
        return self._native.id

    @property
    def stream_channel(self) -> int:
        return self._native.stream_channel

    @property
    def block_id(self) -> int:
        """The block (frame) the event concerns."""
        return self._native.block_id

    @property
    def ts_dev(self) -> int:
        """The camera's timestamp, in its ticks."""
        return self._native.ts_dev

    @property
    def ts_host(self) -> Optional[float]:
        """The timestamp mapped to host time, as POSIX seconds; the time it
        was received when the camera's clock is not calibrated."""
        return self._native.ts_host

    @property
    def data(self) -> bytes:
        """The event's data after its header; empty for most events."""
        return self._native.data

    def __repr__(self) -> str:
        return self._native.__repr__()


class EventStream:
    """Context manager and iterator over a camera's events.

    Open one with :meth:`Camera.events`. Iteration waits up to five seconds
    for each event and raises ``TransportError`` if none comes;
    :meth:`next_event` takes its own timeout.
    """

    __slots__ = ("_native",)

    def __init__(self, native: Any) -> None:
        self._native = native

    @property
    def port(self) -> int:
        """The UDP port the events arrive on."""
        return self._native.port

    def next_event(self, timeout_ms: Optional[int] = None) -> Event:
        """Wait for the next event; ``TransportError`` after ``timeout_ms``."""
        return Event(self._native.next_event(timeout_ms))

    def close(self) -> None:
        """Stop receiving and release the socket."""
        self._native.close()

    def __iter__(self) -> Iterator[Event]:
        return self

    def __next__(self) -> Event:
        return Event(self._native.__next__())

    def __enter__(self) -> "EventStream":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()


__all__ = ["Event", "EventStream"]
