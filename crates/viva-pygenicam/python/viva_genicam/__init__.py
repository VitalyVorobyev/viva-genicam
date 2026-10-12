"""Python bindings for viva-genicam.

Pure-Rust GenICam stack for GigE Vision and USB3 Vision cameras.

Quickstart:

    import viva_genicam as vg

    cams = vg.discover(timeout_ms=500)
    with vg.connect_gige(cams[0]) as cam:
        print(cam.get("DeviceModelName"))

        with cam.stream() as frames:
            for frame in frames:
                arr = frame.to_numpy()
                break
"""

from __future__ import annotations

from .camera import Camera, ControlState, Roi
from .discovery import DeviceInfo, GigeDeviceInfo, U3vDeviceInfo, discover, discover_u3v
from .errors import (
    GenApiError,
    GenicamError,
    MissingChunkFeatureError,
    ParseError,
    TransportError,
    UnsupportedPixelFormatError,
)
from .events import Event, EventStream
from .frame import Frame, FrameStatus
from .network import AckSummary, force_ip, send_action
from .node import FeatureState, NodeInfo, NodeKind, SkippedNode, ValueRange
from .stream import FrameStream, StreamStats

# Lazy functional wrappers so the public API is a flat module.
from .camera import connect_gige, connect_u3v

__all__ = [
    "AckSummary",
    "Camera",
    "ControlState",
    "DeviceInfo",
    "Event",
    "EventStream",
    "FeatureState",
    "Frame",
    "FrameStatus",
    "FrameStream",
    "GigeDeviceInfo",
    "NodeInfo",
    "NodeKind",
    "Roi",
    "SkippedNode",
    "StreamStats",
    "U3vDeviceInfo",
    "ValueRange",
    "GenicamError",
    "GenApiError",
    "TransportError",
    "ParseError",
    "MissingChunkFeatureError",
    "UnsupportedPixelFormatError",
    "discover",
    "discover_u3v",
    "connect_gige",
    "connect_u3v",
    "force_ip",
    "send_action",
]

__version__ = "0.8.0"
