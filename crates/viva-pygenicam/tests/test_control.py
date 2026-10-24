"""Control privilege and monitors from Python (PY-05).

Who holds control is asked of the fake (`controller()`), and what a monitor
wrote is counted by the fake (`writes()`).
"""

from __future__ import annotations

import socket
import struct
import time

import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera

REGS = FakeGigeCamera.REGISTERS


def u32(data: bytes) -> int:
    return struct.unpack(">I", data)[0]


@pytest.fixture()
def camera(fake_gige):
    cam = vg.connect_gige(fake_gige.device_info())
    yield cam
    cam.close()


def test_release_and_reclaim(camera, fake_gige):
    assert camera.control_state is vg.ControlState.HELD
    assert fake_gige.controller() is not None
    camera.release_control()
    assert fake_gige.controller() is None
    assert camera.control_state is vg.ControlState.RELEASED
    with pytest.raises(vg.GenicamError, match="released"):
        camera.set_integer("Width", 320)
    camera.reclaim_control()
    assert fake_gige.controller() is not None
    assert camera.control_state is vg.ControlState.HELD
    camera.set_integer("Width", 320)
    assert u32(fake_gige.peek(REGS["Width"], 4)) == 320
    camera.set_integer("Width", 640)


def test_lost_control_is_reported(camera, fake_gige):
    """The camera drops control on its side, as when a heartbeat expires; the
    keepalive notices, and nothing more is written until it is reclaimed."""
    fake_gige.revoke_control()
    deadline = time.monotonic() + 5
    while camera.control_state is not vg.ControlState.LOST and time.monotonic() < deadline:
        time.sleep(0.05)
    assert camera.control_state is vg.ControlState.LOST
    with pytest.raises(vg.GenicamError, match="lost"):
        camera.set_integer("Width", 320)
    assert u32(fake_gige.peek(REGS["Width"], 4)) == 640, "nothing was written"
    camera.reclaim_control()
    assert fake_gige.controller() is not None
    assert camera.control_state is vg.ControlState.HELD


def test_a_monitor_reports_its_state(fake_gige):
    with vg.connect_gige(fake_gige.device_info(), monitor=True) as monitor:
        assert monitor.control_state is vg.ControlState.MONITOR
        assert fake_gige.controller() is None


def loopback_multicast_works() -> bool:
    """Whether this host delivers a multicast datagram sent on loopback to a
    socket joined on loopback. macOS does; not every CI runner does."""
    group = "239.192.1.99"
    try:
        rx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
        rx.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        rx.bind(("", 0))
        port = rx.getsockname()[1]
        membership = socket.inet_aton(group) + socket.inet_aton("127.0.0.1")
        rx.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, membership)
        rx.settimeout(0.3)
        tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
        tx.bind(("127.0.0.1", 0))
        tx.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton("127.0.0.1"))
        tx.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
        tx.sendto(b"probe", (group, port))
        rx.recvfrom(16)
        return True
    except OSError:
        return False
    finally:
        for sock in ("rx", "tx"):
            if sock in locals():
                locals()[sock].close()


@pytest.mark.skipif(
    not loopback_multicast_works(), reason="loopback does not carry multicast on this host"
)
def test_a_monitor_watches_a_multicast_stream_it_did_not_configure(fake_gige, loopback_iface):
    info = fake_gige.device_info()
    with vg.connect_gige(info) as controller:
        own = controller.stream(iface=loopback_iface, multicast="239.192.1.21")
        with own:
            with vg.connect_gige(info, monitor=True) as monitor:
                writes = fake_gige.writes()
                with monitor.stream(iface=loopback_iface, monitor=True) as watched:
                    got = watched.next_frame(timeout_ms=5000)
                assert got is not None and (got.width, got.height) == (640, 480)
                assert fake_gige.writes() == writes, "the monitor wrote to the camera"


def test_exclusive_access_keeps_other_applications_from_reading(fake_gige):
    """API-22: exclusive=True claims CCP 0x1 on the camera, and while it is
    held a monitor cannot even open it."""
    info = fake_gige.device_info()
    with vg.connect_gige(info, exclusive=True) as cam:
        assert cam.control_state is vg.ControlState.HELD
        assert u32(fake_gige.peek(0x0A00, 4)) == 0x1, "exclusive access on the camera"
        with pytest.raises(vg.GenicamError):
            vg.connect_gige(info, monitor=True)
    assert fake_gige.controller() is None, "released on close"
    with pytest.raises(ValueError, match="one of them"):
        vg.connect_gige(info, monitor=True, exclusive=True)
