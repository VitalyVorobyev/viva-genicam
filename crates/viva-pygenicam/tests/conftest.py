"""Shared pytest fixtures.

Tests use the in-process fake camera from `viva_genicam.testing` —
no subprocess, no external binary to build. One session-scoped fake
serves every test.
"""

from __future__ import annotations

import sys
from typing import Iterator

import pytest

from viva_genicam.testing import FakeGigeCamera


@pytest.fixture(scope="module")
def fake_gige() -> Iterator[FakeGigeCamera]:
    """Boot one in-process fake GigE camera on 127.0.0.1:3956 per test module.

    Module scope (not session) so tests that manage their own fake camera
    lifecycle — see `test_testing.py` — get port 3956 back between
    modules.
    """
    cam = FakeGigeCamera(width=640, height=480, fps=30)
    cam.start()
    try:
        # Make sure discovery actually sees it before tests run.
        cam.device_info(timeout_ms=5000)
        yield cam
    finally:
        cam.stop()


@pytest.fixture()
def loopback_iface() -> str:
    return "lo0" if sys.platform == "darwin" else "lo"
