"""Shared pytest fixtures: spawn the fake GigE camera as a subprocess."""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Iterator

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
FAKE_BIN = REPO_ROOT / "target" / "release" / "viva-fake-gige"


def _wait_for_discovery(deadline: float) -> None:
    """Poll `vg.discover` until the fake camera responds or we time out.

    UDP port probes can't confirm a listener (sendto always succeeds), so
    we do the real thing: call the discovery pipeline and wait until it
    returns a loopback camera.
    """
    import viva_genicam as vg

    last_err: Exception | None = None
    while time.time() < deadline:
        try:
            cams = vg.discover(timeout_ms=400, all=True)
            if any(c.ip.startswith("127.") for c in cams):
                return
        except Exception as exc:
            last_err = exc
        time.sleep(0.1)
    raise RuntimeError(
        f"fake camera not discovered within deadline (last err: {last_err})"
    )


@pytest.fixture(scope="session")
def fake_gige() -> Iterator[None]:
    """Boot one fake GigE camera on 127.0.0.1:3956 for the test session."""
    if not FAKE_BIN.exists():
        pytest.skip(
            f"fake-gige binary not found at {FAKE_BIN} — "
            "run `cargo build -p viva-fake-gige --release`"
        )

    env = os.environ.copy()
    env.setdefault("RUST_LOG", "warn")
    proc = subprocess.Popen(
        [str(FAKE_BIN), "--bind", "127.0.0.1", "--port", "3956"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_for_discovery(deadline=time.time() + 15.0)
        yield
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


@pytest.fixture()
def loopback_iface() -> str:
    return "lo0" if sys.platform == "darwin" else "lo"
