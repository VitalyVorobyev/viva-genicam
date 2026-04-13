"""Shared pytest fixtures: spawn the fake GigE camera as a subprocess."""

from __future__ import annotations

import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Iterator

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
FAKE_BIN = REPO_ROOT / "target" / "release" / "viva-fake-gige"


def _wait_for_port(host: str, port: int, deadline: float) -> None:
    while time.time() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.settimeout(0.1)
            try:
                s.sendto(b"\x00\x00\x00\x00", (host, port))
                return
            except OSError:
                time.sleep(0.05)
    raise RuntimeError(f"fake camera did not bind {host}:{port}")


@pytest.fixture(scope="session")
def fake_gige() -> Iterator[None]:
    """Boot one fake GigE camera on 127.0.0.1:3956 for the test session."""
    if not FAKE_BIN.exists():
        pytest.skip(f"fake-gige binary not found at {FAKE_BIN} — run `cargo build -p viva-fake-gige --release`")

    env = os.environ.copy()
    env.setdefault("RUST_LOG", "warn")
    proc = subprocess.Popen(
        [str(FAKE_BIN), "--bind", "127.0.0.1", "--port", "3956"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_for_port("127.0.0.1", 3956, time.time() + 5.0)
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
