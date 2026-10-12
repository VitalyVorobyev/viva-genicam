"""Frames that lost packets: counted always, handed out when asked (PY-03).

The fake never sends packet 3 of block 2 and does not resend, so block 2 can
only close incomplete — by the fake's doing, not the receiver's.
"""

from __future__ import annotations

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera

DROPPED_BLOCK = 2


def frames_up_to(cam: vg.Camera, last_block: int, **stream_args) -> tuple[list[vg.Frame], vg.StreamStats]:
    got: list[vg.Frame] = []
    with cam.stream(packet_size=1500, resend=False, **stream_args) as frames:
        # Bounded: a stream whose block ids never reach `last_block` fails
        # below rather than hanging here.
        while len(got) < 20 and (not got or got[-1].block_id < last_block):
            got.append(frames.next_frame(timeout_ms=5000))
        stats = frames.stats()
    return got, stats


def test_an_incomplete_frame_is_delivered_when_asked_for():
    with FakeGigeCamera(fps=20, packet_resend=False, drop_packets=[(DROPPED_BLOCK, 3)]) as fake:
        cam = vg.connect_gige(fake.device_info())
        got, stats = frames_up_to(cam, DROPPED_BLOCK + 1, deliver_incomplete=True)
        cam.close()
    damaged = [f for f in got if f.block_id == DROPPED_BLOCK]
    assert len(damaged) == 1
    frame = damaged[0]
    assert frame.status is vg.FrameStatus.INCOMPLETE
    assert not frame.is_complete
    assert frame.missing_packets == 1
    assert all(f.status is vg.FrameStatus.COMPLETE for f in got if f.block_id != DROPPED_BLOCK)
    assert stats.frames_incomplete == 1
    assert stats.packets_missing == 1


def test_an_incomplete_frame_is_counted_but_not_delivered_by_default():
    with FakeGigeCamera(fps=20, packet_resend=False, drop_packets=[(DROPPED_BLOCK, 3)]) as fake:
        cam = vg.connect_gige(fake.device_info())
        got, stats = frames_up_to(cam, DROPPED_BLOCK + 1)
        cam.close()
    assert DROPPED_BLOCK not in [f.block_id for f in got]
    assert stats.frames_incomplete == 1
    assert stats.frames_lost >= 1
