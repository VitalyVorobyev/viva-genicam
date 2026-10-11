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
    with camera.stream(packet_size=1500) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None
        assert frame.width > 0
        assert frame.height > 0
        assert frame.pixel_format


def test_frame_to_numpy_shape(camera):
    expected_w = int(camera.get("Width"))
    expected_h = int(camera.get("Height"))
    with camera.stream(packet_size=1500) as frames:
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
    with camera.stream(packet_size=1500) as frames:
        for frame in frames:
            received += 1
            if received >= 3:
                break
    assert received == 3


def test_stream_auto_iface_for_loopback(fake_gige):
    """Default `cam.stream()` with no iface should resolve via subnet match.

    For the loopback fake camera this should pick the lo/lo0 interface
    automatically — no need for the user to pass `iface=`.
    """
    cams = vg.discover(timeout_ms=1500, all=True)
    cam_info = next(c for c in cams if c.ip.startswith("127."))
    cam = vg.connect_gige(cam_info)  # no iface
    with cam.stream(packet_size=1500) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None


def test_auto_packet_size(camera):
    with camera.stream(auto_packet_size=True) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None


def test_auto_and_packet_size_conflict(camera):
    with pytest.raises(Exception, match="at most one"):
        with camera.stream(auto_packet_size=True, packet_size=1500):
            pass


def test_ts_host_is_none_until_calibrated(camera):
    """Without a clock fit there is no host time to give (backlog DOC-11).

    Every GigE frame used to carry the wall-clock time it was handed over,
    passed off as its device time mapped to the host.
    """
    with camera.stream(packet_size=1500) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame.ts_host is None


def test_ts_host_follows_time_calibrate(camera):
    import time

    camera.time_calibrate(samples=4, interval_ms=10)
    with camera.stream(packet_size=1500) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame.ts_host is not None
        assert abs(frame.ts_host - time.time()) < 5


def test_packet_delay_and_resend_options(camera):
    """``packet_delay_ns`` and ``resend`` reach ``open_stream``: a delay the
    fake converts to its ticks still streams, and so does a stream that never
    asks for lost packets."""
    with camera.stream(packet_size=1500, packet_delay_ns=2000, resend=False) as frames:
        frame = frames.next_frame(timeout_ms=5000)
        assert frame is not None
