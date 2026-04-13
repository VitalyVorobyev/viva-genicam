"""Python bindings for viva-genicam.

Pure-Rust GenICam stack for GigE Vision and USB3 Vision cameras.

Quickstart:

    import viva_genicam as vg

    cams = vg.discover(timeout_ms=500)
    cam = vg.connect_gige(cams[0])
    print(cam.get("DeviceModelName"))

    with cam.stream() as frames:
        for frame in frames:
            arr = frame.to_numpy()
            break
"""

from __future__ import annotations

from .camera import Camera
from .discovery import DeviceInfo, GigeDeviceInfo, U3vDeviceInfo, discover, discover_u3v
from .errors import (
    GenApiError,
    GenicamError,
    MissingChunkFeatureError,
    ParseError,
    TransportError,
    UnsupportedPixelFormatError,
)
from .frame import Frame
from .node import NodeInfo, NodeKind
from .stream import FrameStream

# Lazy functional wrappers so the public API is a flat module.
from .camera import connect_gige, connect_u3v

__all__ = [
    "Camera",
    "DeviceInfo",
    "GigeDeviceInfo",
    "U3vDeviceInfo",
    "Frame",
    "FrameStream",
    "NodeInfo",
    "NodeKind",
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
]

__version__ = "0.2.4"
