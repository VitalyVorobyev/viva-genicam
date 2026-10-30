"""Event-vision blocks through ``FrameStream.next_block``.

The fake streams EVT data when its pixel format is an EVT code, as a LUCID
Triton2 EVS does: an image leader whose Size X and Size Y are not an image's,
and the encoded words as the payload — one byte per "pixel" on the fake.
"""

from __future__ import annotations

import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera, FakeU3vCamera

WIDTH, HEIGHT = 16, 4


@pytest.mark.parametrize(
    "pixel_format, evt",
    [("EVT3_0", "evt3.0"), ("EVT2_1", "evt2.1")],
)
def test_next_block_hands_out_event_data_as_an_evs_block(pixel_format, evt):
    with FakeGigeCamera(width=WIDTH, height=HEIGHT, pixel_format=pixel_format) as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream(packet_size=1500) as frames:
            block = frames.next_block(timeout_ms=5000)
        cam.close()
    assert isinstance(block, vg.EvsBlock)
    assert block.format == evt
    assert len(block.payload()) == WIDTH * HEIGHT
    assert (block.leader_size_x, block.leader_size_y) == (WIDTH, HEIGHT)
    assert block.trailer_size_y == HEIGHT
    assert block.status is vg.FrameStatus.COMPLETE
    assert block.block_id >= 1
    assert block.ts_dev is not None
    assert block.ts_host is None
    assert block.chunks is None
    assert evt in repr(block)


def test_next_frame_still_hands_out_event_data_as_a_frame():
    with FakeGigeCamera(width=WIDTH, height=HEIGHT, pixel_format="EVT3_0") as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream(packet_size=1500) as frames:
            frame = frames.next_frame(timeout_ms=5000)
        cam.close()
    assert isinstance(frame, vg.Frame)
    assert frame.pixel_format_code == 0x8110_0E30
    assert (frame.width, frame.height) == (WIDTH, HEIGHT)
    assert len(frame.payload()) == WIDTH * HEIGHT


def test_next_block_hands_out_an_image_as_a_frame():
    with FakeGigeCamera(width=WIDTH, height=HEIGHT) as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream(packet_size=1500) as frames:
            block = frames.next_block(timeout_ms=5000)
        cam.close()
    assert isinstance(block, vg.Frame)
    assert (block.width, block.height, block.pixel_format) == (WIDTH, HEIGHT, "Mono8")


def test_a_u3v_stream_hands_out_frames_from_next_block():
    fake = FakeU3vCamera(width=64, height=48)
    camera = fake.connect()
    with camera.stream() as frames:
        block = frames.next_block(timeout_ms=5000)
    assert isinstance(block, vg.Frame)
    assert (block.width, block.height) == (64, 48)
