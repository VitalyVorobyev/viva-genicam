"""USB3 Vision through the bindings, against the in-process fake camera."""

from __future__ import annotations

import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeU3vCamera


def test_a_u3v_stream_starts_and_stops_the_camera():
    fake = FakeU3vCamera(width=64, height=48)
    camera = fake.connect()
    assert camera.get("Width") == "64"
    with camera.stream() as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert fake.acquiring(), "the stream started acquisition on the camera"
    assert (frame.width, frame.height) == (64, 48)
    assert not fake.acquiring(), "closing the stream stopped it"


def test_monitor_is_refused_on_usb3_vision():
    """USB3 Vision has no read-only access to a camera another application
    controls: one application opens the device. A monitor stream would wait
    for an acquisition nobody starts."""
    fake = FakeU3vCamera()
    camera = fake.connect()
    with pytest.raises(vg.GenicamError, match="GigE Vision only"):
        camera.stream(monitor=True)
    assert not fake.acquiring()


def test_a_u3v_stream_counts_what_it_reads():
    """USB3 Vision streams have the same counters as GigE Vision's."""
    fake = FakeU3vCamera(width=64, height=48)
    camera = fake.connect()
    with camera.stream() as frames:
        for _ in range(3):
            frames.next_frame(timeout_ms=5000)
        stats = frames.stats()
    assert stats.frames_complete >= 3
    assert stats.bytes == stats.frames_complete * 64 * 48
    assert stats.packets_received > 0
    assert stats.transfer_errors == 0


def test_u3v_events_arrive_from_the_event_interface():
    """USB3 Vision events read through the camera's event ports."""
    fake = FakeU3vCamera(width=64, height=48)
    camera = fake.connect()
    events = camera.events(["ExposureEnd"])
    with camera.stream() as frames:
        frames.next_frame(timeout_ms=5000)
        event = events.next_event(timeout_ms=5000)
    assert event.id == 0x9001
    features = camera.event_features(event)
    assert features["EventExposureEnd"] == str(0x9001)
    assert int(features["EventExposureEndTimestamp"]) == event.ts_dev
    events.close()
