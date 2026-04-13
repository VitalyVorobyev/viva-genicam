"""API discoverability: signatures, exports, type marker."""

from __future__ import annotations

import importlib
import inspect
from pathlib import Path

import viva_genicam as vg


def test_all_exports_exist():
    for name in vg.__all__:
        assert hasattr(vg, name), f"missing export: {name}"


def test_py_typed_present():
    pkg_path = Path(vg.__file__).parent
    assert (pkg_path / "py.typed").is_file()


def test_public_signatures_are_real():
    # These must be inspectable (not *args/**kwargs wrappers).
    assert list(inspect.signature(vg.discover).parameters) == [
        "timeout_ms",
        "iface",
        "all",
    ]
    assert list(inspect.signature(vg.connect_gige).parameters) == [
        "device_info",
        "iface",
    ]
    assert list(inspect.signature(vg.connect_u3v).parameters) == ["device_info"]
    # discover_u3v takes no args — just confirm it's introspectable.
    inspect.signature(vg.discover_u3v)


def test_native_module_is_private():
    # Users should never need to import _native.
    assert "_native" not in vg.__all__


def test_camera_has_expected_methods():
    for attr in (
        "get",
        "set",
        "set_exposure_time_us",
        "set_gain_db",
        "nodes",
        "node_info",
        "all_node_info",
        "categories",
        "acquisition_start",
        "acquisition_stop",
        "stream",
    ):
        assert hasattr(vg.Camera, attr), f"Camera missing {attr}"
