"""`Frame.to_numpy`: the camera's values, in the shape and type of its format (PY-02).

The frames are built by hand from the layouts the standards give, so every
expected value below was worked out from the specification, not from the
decoder:

- GigE Vision 2.0 `Mono12Packed`: two pixels in three bytes — byte 0 holds
  pixel 0's bits 11..4, byte 1 pixel 0's bits 3..0 in its low nibble and
  pixel 1's bits 3..0 in its high nibble, byte 2 pixel 1's bits 11..4.
- PFNC 2.4 `Mono12p`, `Mono10p`, `BayerRG12p`: pixels packed into a
  little-endian bit stream, least significant bit first (§6.3.1).
- PFNC unpacked `Mono10`/`Mono14`/`Mono16`: one little-endian 16-bit word per
  pixel (§6.1.1).
- GVSP `padding_x`: bytes after every line, which are not pixels.
"""

from __future__ import annotations

import numpy as np
import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera


def frame(payload: bytes, width: int, height: int, fmt: str, padding_x: int = 0) -> vg.Frame:
    return vg.Frame.from_bytes(bytes(payload), width, height, fmt, padding_x)


MONO12_VALUES = [[0x000, 0xFFF, 0x800, 0x001], [0xABC, 0x123, 0x7FF, 0x400]]


def test_mono12packed_is_two_dimensional_uint16():
    payload = [
        0x00, 0xF0, 0xFF,  # 0x000, 0xFFF
        0x80, 0x10, 0x00,  # 0x800, 0x001
        0xAB, 0x3C, 0x12,  # 0xABC, 0x123
        0x7F, 0x0F, 0x40,  # 0x7FF, 0x400
    ]
    arr = frame(payload, 4, 2, "Mono12Packed").to_numpy()
    assert arr.dtype == np.uint16
    assert arr.shape == (2, 4)
    assert arr.tolist() == MONO12_VALUES


def test_mono12p_is_two_dimensional_uint16():
    payload = [
        0x00, 0xF0, 0xFF,  # 0x000, 0xFFF
        0x00, 0x18, 0x00,  # 0x800, 0x001
        0xBC, 0x3A, 0x12,  # 0xABC, 0x123
        0xFF, 0x07, 0x40,  # 0x7FF, 0x400
    ]
    arr = frame(payload, 4, 2, "Mono12p").to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint16, (2, 4))
    assert arr.tolist() == MONO12_VALUES


def test_mono10p_is_two_dimensional_uint16():
    # 0x3FF | 0x001 << 10 | 0x200 << 20 | 0x155 << 30, five bytes little-endian.
    arr = frame([0xFF, 0x07, 0x00, 0x60, 0x55], 4, 1, "Mono10p").to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint16, (1, 4))
    assert arr.tolist() == [[0x3FF, 0x001, 0x200, 0x155]]


@pytest.mark.parametrize(
    "fmt, payload, values",
    [
        ("Mono10", [0xFF, 0x03, 0x00, 0x00, 0x55, 0x01], [0x3FF, 0x000, 0x155]),
        ("Mono14", [0xFF, 0x3F, 0x00, 0x20, 0x01, 0x00], [0x3FFF, 0x2000, 0x0001]),
    ],
)
def test_unpacked_deep_mono_is_two_dimensional_uint16(fmt, payload, values):
    arr = frame(payload, 3, 1, fmt).to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint16, (1, 3))
    assert arr.tolist() == [values]


def test_line_padding_is_not_pixels():
    """`padding_x` bytes after each line are skipped, at every depth."""
    mono16 = [0x02, 0x01, 0xFF, 0xFF, 0xAA, 0xAA, 0x00, 0x80, 0x01, 0x00, 0xBB, 0xBB]
    arr = frame(mono16, 2, 2, "Mono16", padding_x=2).to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint16, (2, 2))
    assert arr.tolist() == [[0x0102, 0xFFFF], [0x8000, 0x0001]]

    mono8 = [1, 2, 3, 0xEE, 4, 5, 6, 0xEE]
    arr = frame(mono8, 3, 2, "Mono8", padding_x=1).to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint8, (2, 3))
    assert arr.tolist() == [[1, 2, 3], [4, 5, 6]]


def test_colour_is_height_by_width_by_three():
    rgb = [1, 2, 3, 4, 5, 6, 0xEE, 0xEE, 7, 8, 9, 10, 11, 12, 0xEE, 0xEE]
    arr = frame(rgb, 2, 2, "RGB8", padding_x=2).to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint8, (2, 2, 3))
    assert arr.tolist() == [[[1, 2, 3], [4, 5, 6]], [[7, 8, 9], [10, 11, 12]]]

    bgr = [3, 2, 1, 6, 5, 4]
    arr = frame(bgr, 2, 1, "BGR8").to_numpy()
    assert arr.tolist() == [[[1, 2, 3], [4, 5, 6]]], "BGR is reordered to RGB"


def test_bayer_is_the_mosaic_unless_demosaicing_is_asked_for():
    # BayerRG: R G / G B. The mosaic as the camera sent it.
    bayer8 = frame([200, 10, 20, 90], 2, 2, "BayerRG8")
    arr = bayer8.to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint8, (2, 2))
    assert arr.tolist() == [[200, 10], [20, 90]]

    rgb = bayer8.to_numpy(demosaic=True)
    assert (rgb.dtype, rgb.shape) == (np.uint8, (2, 2, 3))
    # A sample's own colour is kept where it was measured.
    assert rgb[0, 0, 0] == 200, "red at the red site"
    assert rgb[1, 1, 2] == 90, "blue at the blue site"


def test_deep_bayer_keeps_its_depth():
    # BayerRG12p: 0xFFF, 0x001 / 0x800, 0x123.
    bayer12 = frame([0xFF, 0x1F, 0x00, 0x00, 0x38, 0x12], 2, 2, "BayerRG12p")
    arr = bayer12.to_numpy()
    assert (arr.dtype, arr.shape) == (np.uint16, (2, 2))
    assert arr.tolist() == [[0xFFF, 0x001], [0x800, 0x123]]

    rgb = bayer12.to_numpy(demosaic=True)
    assert (rgb.dtype, rgb.shape) == (np.uint16, (2, 2, 3))
    assert rgb[0, 0, 0] == 0xFFF, "red at the red site, at full depth"
    assert rgb[1, 1, 2] == 0x123, "blue at the blue site, at full depth"


def test_demosaic_leaves_monochrome_alone():
    arr = frame([0x02, 0x01, 0xFF, 0xFF], 2, 1, "Mono16").to_numpy(demosaic=True)
    assert (arr.dtype, arr.shape) == (np.uint16, (1, 2))


def test_a_streamed_mono12p_frame_is_uint16():
    """The same over the wire: the fake streams its test pattern as Mono12p."""
    with FakeGigeCamera(width=64, height=8, fps=20, pixel_format="Mono12p") as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream(packet_size=1500) as frames:
            got = frames.next_frame(timeout_ms=5000)
        assert got is not None and got.pixel_format == "Mono12p"
        arr = got.to_numpy()
        assert (arr.dtype, arr.shape) == (np.uint16, (8, 64))
        assert int(arr.max()) <= 0xFFF, "twelve significant bits"
        del cam
