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


def test_discover_ip_asks_one_camera_directly(fake_gige):
    """PY-06: unicast discovery, for a camera broadcasts do not reach."""
    found = vg.discover_ip(fake_gige.ip, timeout_ms=1500)
    assert found is not None
    assert found.ip == fake_gige.ip
    assert found.mac == fake_gige.MAC
    before = len(fake_gige.discovery_flags())
    vg.discover_ip(fake_gige.ip, timeout_ms=500)
    assert len(fake_gige.discovery_flags()) == before + 1, "one command, to the camera"


def test_allow_broadcast_ack_is_asked_for_only_when_given(fake_gige):
    """PY-06: the discovery command carries the allow-broadcast-ack flag
    (0x10) when asked, and not otherwise; the fake records each command's
    flags as it received them."""
    before = len(fake_gige.discovery_flags())
    vg.discover(timeout_ms=300, iface=fake_gige.ip)
    plain = fake_gige.discovery_flags()[before:]
    assert plain and all(flags & 0x10 == 0 for flags in plain), plain
    before = len(fake_gige.discovery_flags())
    cams = vg.discover(timeout_ms=1500, iface=fake_gige.ip, allow_broadcast_ack=True)
    allowed = fake_gige.discovery_flags()[before:]
    assert allowed and all(flags & 0x10 for flags in allowed), allowed
    assert any(c.ip == fake_gige.ip for c in cams)
