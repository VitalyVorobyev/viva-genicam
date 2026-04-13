"""Exception hierarchy for viva-genicam.

All exceptions raised by the native extension subclass ``GenicamError``.
They are defined on the Rust side and re-exported here so users can do
``except vg.TransportError:`` without reaching into ``_native``.
"""

from __future__ import annotations

from . import _native

GenicamError = _native.GenicamError
GenApiError = _native.GenApiError
TransportError = _native.TransportError
ParseError = _native.ParseError
MissingChunkFeatureError = _native.MissingChunkFeatureError
UnsupportedPixelFormatError = _native.UnsupportedPixelFormatError

__all__ = [
    "GenicamError",
    "GenApiError",
    "TransportError",
    "ParseError",
    "MissingChunkFeatureError",
    "UnsupportedPixelFormatError",
]
