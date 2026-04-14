"""The `viva_genicam.testing` in-process fake-camera binding."""

from __future__ import annotations

import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera


def test_context_manager_lifecycle():
    """start/stop via `with` and the fake is discoverable while running."""
    with FakeGigeCamera(width=320, height=240, fps=10) as fake:
        info = fake.device_info()
        assert info.ip.startswith("127.")
        assert info.transport == "gige"


def test_device_info_before_start_raises():
    fake = FakeGigeCamera()
    with pytest.raises(vg.TransportError):
        fake.device_info(timeout_ms=200)


def test_double_start_is_idempotent():
    fake = FakeGigeCamera()
    try:
        fake.start()
        fake.start()  # must not raise "address in use"
        info = fake.device_info()
        assert info.ip.startswith("127.")
    finally:
        fake.stop()


def test_connect_and_stream_through_fake():
    """Smoke test the full pipeline against the in-process fake."""
    with FakeGigeCamera(width=320, height=240, fps=15) as fake:
        cam = vg.connect_gige(fake.device_info())
        assert cam.transport == "gige"
        assert int(cam.get("Width")) == 320
        with cam.stream(auto_packet_size=False) as frames:
            frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None
        assert frame.width == 320
        assert frame.height == 240


def test_repr_reflects_state():
    fake = FakeGigeCamera(width=320, height=240)
    assert "stopped" in repr(fake)
    fake.start()
    try:
        assert "running" in repr(fake)
    finally:
        fake.stop()
