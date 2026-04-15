"""Feature get/set + introspection."""

from __future__ import annotations

import pytest

import viva_genicam as vg


@pytest.fixture()
def camera(fake_gige):
    cams = vg.discover(timeout_ms=1500, all=True)
    cam_info = next(c for c in cams if c.ip.startswith("127."))
    return vg.connect_gige(cam_info)


def test_camera_metadata(camera):
    assert camera.transport == "gige"
    assert "RegisterDescription" in camera.xml or "Category" in camera.xml


def test_read_width_and_height(camera):
    w = int(camera.get("Width"))
    h = int(camera.get("Height"))
    assert w > 0 and h > 0


def test_read_exposure_is_float(camera):
    v = float(camera.get("ExposureTime"))
    assert v > 0


def test_set_exposure_via_typed_helper(camera):
    camera.set_exposure_time_us(7500.0)
    v = float(camera.get("ExposureTime"))
    assert abs(v - 7500.0) < 1.0


def test_set_width_roundtrip(camera):
    original = int(camera.get("Width"))
    new = 128 if original > 128 else 256
    camera.set("Width", str(new))
    assert int(camera.get("Width")) == new
    camera.set("Width", str(original))


def test_node_introspection(camera):
    names = camera.nodes()
    assert "Width" in names
    info = camera.node_info("Width")
    assert info is not None
    assert info.kind == "Integer"
    assert info.access in {"RW", "RO"}


def test_categories_non_empty(camera):
    cats = camera.categories()
    assert isinstance(cats, dict)
    assert len(cats) > 0


def test_unknown_feature_raises(camera):
    with pytest.raises(vg.GenApiError):
        camera.get("NonExistent_Feature_XYZ")
