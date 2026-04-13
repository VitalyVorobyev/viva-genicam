"""End-to-end streaming through the fake GigE camera."""

from __future__ import annotations

import numpy as np
import pytest

import viva_genicam as vg


@pytest.fixture()
def camera(fake_gige, loopback_iface):
    cams = vg.discover(timeout_ms=1500, all=True)
    cam_info = next(c for c in cams if c.ip.startswith("127."))
    return vg.connect_gige(cam_info, iface=loopback_iface)


def test_single_frame(camera):
    with camera.stream(auto_packet_size=False) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None
        assert frame.width > 0
        assert frame.height > 0
        assert frame.pixel_format


def test_frame_to_numpy_shape(camera):
    expected_w = int(camera.get("Width"))
    expected_h = int(camera.get("Height"))
    with camera.stream(auto_packet_size=False) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        arr = frame.to_numpy()
        assert isinstance(arr, np.ndarray)
        assert frame.width == expected_w
        assert frame.height == expected_h
        # Mono8 default → (H, W) uint8
        if frame.pixel_format.lower().startswith("mono8"):
            assert arr.shape == (expected_h, expected_w)
            assert arr.dtype == np.uint8


def test_iterate_multiple_frames(camera):
    received = 0
    with camera.stream(auto_packet_size=False) as frames:
        for frame in frames:
            received += 1
            if received >= 3:
                break
    assert received == 3
