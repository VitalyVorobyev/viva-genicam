"""GigE Vision commands sent to the network, not to one open camera."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from . import _native


@dataclass(frozen=True)
class AckSummary:
    """What an action command reached."""

    #: Datagrams sent.
    sent: int
    #: Cameras that acknowledged: those whose keys matched.
    acks: int


def send_action(
    device_key: int,
    group_key: int,
    group_mask: int,
    scheduled_time: Optional[int] = None,
    destination: str = "255.255.255.255",
    timeout_ms: int = 200,
) -> AckSummary:
    """Send a GigE Vision action command and count the acknowledgements.

    Every camera that receives it compares the keys with its own
    ``ActionDeviceKey``, ``ActionGroupKey`` and ``ActionGroupMask``; those that
    match act, and acknowledge within ``timeout_ms``. ``destination`` is the
    broadcast address by default, or one camera's address.
    ``scheduled_time``, in the cameras' timestamp ticks, asks them to act then
    — only cameras that support scheduled actions accept it.
    """
    sent, acks = _native.send_action(
        device_key, group_key, group_mask, scheduled_time, destination, timeout_ms
    )
    return AckSummary(sent=sent, acks=acks)


def force_ip(
    mac: str,
    ip: str,
    subnet: str,
    gateway: str = "0.0.0.0",
    iface: Optional[str] = None,
) -> None:
    """Give the camera with ``mac`` (``"DE:AD:BE:EF:CA:FE"``) a temporary
    address with a broadcast FORCEIP, for a camera configured for another
    network. It lasts until the camera restarts; :meth:`Camera.set_persistent_ip`
    stores one. ``iface`` picks the host interface to broadcast on."""
    _native.force_ip(mac, ip, subnet, gateway, iface)


__all__ = ["AckSummary", "send_action", "force_ip"]
