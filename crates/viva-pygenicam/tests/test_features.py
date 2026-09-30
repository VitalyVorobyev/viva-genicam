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
    # `access` is the XML declaration; `effective_access` is what the device
    # permits now. Single-node lookups resolve it.
    assert info.effective_access in {"RW", "RO", "WO"}


def test_effective_access_reflects_the_device_lock(camera):
    """`ExposureTime` is locked by `ExposureAuto` in the fake camera's XML.

    Reporting only the static XML access mode told #45's reporter their
    `ExposureTime` was writable while the camera refused every write to it.
    """
    camera.set("ExposureAuto", "Off")
    info = camera.node_info("ExposureTime")
    assert info is not None
    assert info.effective_access == "RW"

    camera.set("ExposureAuto", "Continuous")
    info = camera.node_info("ExposureTime")
    assert info is not None
    assert info.effective_access == "RO", "a locked node must not report writable"
    # The static declaration is unchanged -- both values are meaningful.
    assert info.access == "RW"

    # And the write itself is refused locally, naming the lock.
    with pytest.raises(Exception) as excinfo:
        camera.set_exposure_time_us(10_000.0)
    assert "ExposureTime" in str(excinfo.value)

    camera.set("ExposureAuto", "Off")


def test_all_node_info_leaves_effective_access_unresolved(camera):
    """Bulk introspection must not do a register read per node."""
    infos = camera.all_node_info()
    assert len(infos) > 0
    assert all(i.effective_access is None for i in infos)


def test_categories_non_empty(camera):
    cats = camera.categories()
    assert isinstance(cats, dict)
    assert len(cats) > 0


def test_unknown_feature_raises(camera):
    with pytest.raises(vg.GenApiError):
        camera.get("NonExistent_Feature_XYZ")


def test_execute_runs_a_command_and_changes_the_camera(camera):
    """`execute` on a `<Command>` node, asserted against the camera's state.

    This is the workflow from issue #121: select a user set, load it. The
    exposure is moved off its default first, so an execute that quietly did
    nothing would fail here rather than pass on a bare `Ok`.

    `UserSetLoad` in the fake reaches its register through `<pValue>`, which is
    the shape all 432 `<Command>` nodes in the vendor XML corpus use.
    """
    assert camera.node_info("UserSetLoad").kind == "Command"
    assert "Default" in camera.enum_entries("UserSetSelector")

    camera.set("ExposureTime", "20000.0")
    assert camera.get("ExposureTime").startswith("20000")

    camera.set("UserSetSelector", "Default")
    camera.execute("UserSetLoad")

    # The open camera re-reads: the fake's exposure register names
    # UserSetLoad's register in <pInvalidator>, as FLIR's XML does (GA-24).
    assert camera.get("ExposureTime").startswith("5000")

    # And a fresh nodemap agrees, so the device itself was reset.
    cams = vg.discover(timeout_ms=1500, all=True)
    reopened = vg.connect_gige(next(c for c in cams if c.ip.startswith("127.")))
    assert reopened.get("ExposureTime").startswith("5000")


def test_execute_rejects_a_non_command_node(camera):
    """A non-Command node must be refused, not silently written."""
    with pytest.raises(Exception):
        camera.execute("ExposureTime")
