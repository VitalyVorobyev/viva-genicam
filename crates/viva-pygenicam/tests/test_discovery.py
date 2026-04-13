"""Discovery surface."""

from __future__ import annotations

import viva_genicam as vg


def test_discover_returns_list(fake_gige):
    cams = vg.discover(timeout_ms=1500, all=True)
    assert isinstance(cams, list)
    loopback = [c for c in cams if c.ip.startswith("127.")]
    assert loopback, f"expected loopback camera, got {[c.ip for c in cams]}"


def test_device_info_to_dict_roundtrip(fake_gige):
    cams = vg.discover(timeout_ms=1500, all=True)
    cam = next(c for c in cams if c.ip.startswith("127."))
    d = cam.to_dict()
    assert d["ip"] == cam.ip
    assert d["mac"] == cam.mac
    assert d["transport"] == "gige"


def test_device_info_repr(fake_gige):
    cams = vg.discover(timeout_ms=1500, all=True)
    assert "GigeDeviceInfo" in repr(cams[0])
