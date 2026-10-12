"""What the Rust `Camera` offers, from Python (PY-03).

Every write is checked against the fake's own memory (`peek`) and every read
against what the fake holds, never against the library's echo of itself.
Register addresses and keys are the fake's (`FakeGigeCamera.REGISTERS`).
"""

from __future__ import annotations

import socket
import struct
import time

import pytest

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera

REGS = FakeGigeCamera.REGISTERS
# The fake's chunk IDs, as its XML declares them on its chunk ports.
CHUNK_TIMESTAMP = 0x0000_0002
CHUNK_FRAME_ID = 0x0504_0003
# The fake's ExposureEnd event, `<EventID>9001</EventID>` in its XML.
EVENT_EXPOSURE_END = 0x9001


def u32(data: bytes) -> int:
    return struct.unpack(">I", data)[0]


def f64(data: bytes) -> float:
    return struct.unpack(">d", data)[0]


def f32(data: bytes) -> float:
    return struct.unpack(">f", data)[0]


@pytest.fixture()
def camera(fake_gige):
    cam = vg.connect_gige(fake_gige.device_info())
    yield cam
    cam.close()


# ── typed features ─────────────────────────────────────────────────────────


def test_integers_by_type(camera, fake_gige):
    camera.set_integer("Width", 320)
    assert u32(fake_gige.peek(REGS["Width"], 4)) == 320
    assert camera.get_integer("Height") == u32(fake_gige.peek(REGS["Height"], 4))
    camera.set_integer("Width", 640)


def test_floats_by_type(camera, fake_gige):
    camera.set_float("ExposureTime", 12345.5)
    assert f64(fake_gige.peek(REGS["ExposureTime"], 8)) == 12345.5
    camera.set_float("Gain", 6.25)
    assert f64(fake_gige.peek(REGS["Gain"], 8)) == 6.25
    assert camera.get_float("Gain") == 6.25


def test_booleans_and_enumerations_by_type(camera, fake_gige):
    camera.set_bool("AcquisitionFrameRateEnable", True)
    assert u32(fake_gige.peek(REGS["AcquisitionFrameRateEnable"], 4)) == 1
    assert camera.get_bool("AcquisitionFrameRateEnable") is True
    camera.set_bool("AcquisitionFrameRateEnable", False)
    assert u32(fake_gige.peek(REGS["AcquisitionFrameRateEnable"], 4)) == 0

    camera.set_enum("AcquisitionMode", "SingleFrame")
    assert u32(fake_gige.peek(REGS["AcquisitionMode"], 4)) == 1
    assert camera.get_enum("AcquisitionMode") == "SingleFrame"
    camera.set_enum("AcquisitionMode", "Continuous")


def test_strings_and_registers_by_type(camera, fake_gige):
    model = fake_gige.peek(REGS["DeviceModelName"], 32).rstrip(b"\0").decode()
    assert camera.get_string("DeviceModelName") == model

    camera.set_string("DeviceUserID", "bench-3")
    assert fake_gige.peek(REGS["DeviceUserID"], 16) == b"bench-3" + b"\0" * 9
    assert camera.get_string("DeviceUserID") == "bench-3"

    data = bytes(range(0xA0, 0xB0))
    camera.set_register("DeviceUserBuffer", data)
    assert fake_gige.peek(REGS["DeviceUserBuffer"], 16) == data
    assert camera.get_register("DeviceUserBuffer") == data
    with pytest.raises(vg.GenicamError):
        camera.set_register("DeviceUserBuffer", b"\x01")


def test_feature_state_reads_everything_at_once(camera, fake_gige):
    state = camera.feature_state("Width")
    assert isinstance(state, vg.FeatureState)
    assert state.kind == "Integer"
    assert state.value == u32(fake_gige.peek(REGS["Width"], 4))
    assert state.access == "RW"
    assert state.is_implemented and state.is_available
    assert state.range is not None and state.range.min >= 1

    temperature = camera.feature_state("DeviceTemperature")
    assert temperature.polling_time == 0.5, "the fake's <PollingTime> of 500 ms"
    assert temperature.caching == "NoCache"
    assert temperature.unit == "C"


def test_available_enum_entries_follow_the_cameras_predicates(camera, fake_gige):
    """`PixelFormat`'s entries are gated by `SensorType` in the fake's XML:
    Monochrome offers Mono8 and Mono16, BayerRG only BayerRG8."""
    assert set(camera.enum_entries("PixelFormat")) == {"Mono8", "Mono16", "RGB8", "BayerRG8"}
    camera.set("SensorType", "BayerRG")
    assert u32(fake_gige.peek(REGS["SensorType"], 4)) == 1
    assert camera.available_enum_entries("PixelFormat") == ["BayerRG8"]
    camera.set("SensorType", "Monochrome")
    assert camera.available_enum_entries("PixelFormat") == ["Mono8", "Mono16"]


# ── refresh ────────────────────────────────────────────────────────────────


def test_changed_since_names_what_a_write_touched(camera):
    before = camera.generation()
    camera.set_integer("Width", 320)
    changed, now = camera.changed_since(before)
    assert now > before
    assert "Width" in changed
    assert camera.changed_since(now) == ([], now)
    camera.set_integer("Width", 640)


def test_poll_reports_a_value_once_its_polling_time_has_passed(camera):
    camera.get("DeviceTemperature")
    assert "DeviceTemperature" not in camera.poll()
    time.sleep(0.6)
    assert "DeviceTemperature" in camera.poll()


# ── standard features ──────────────────────────────────────────────────────


def test_exposure_gain_and_acquisition_mode(camera, fake_gige):
    assert camera.set_exposure_time(8000.0) == "ExposureTime"
    assert camera.exposure_time() == f64(fake_gige.peek(REGS["ExposureTime"], 8)) == 8000.0
    assert camera.set_gain(3.5) == "Gain"
    assert camera.gain() == f64(fake_gige.peek(REGS["Gain"], 8)) == 3.5
    camera.set_acquisition_mode("MultiFrame")
    assert u32(fake_gige.peek(REGS["AcquisitionMode"], 4)) == 2
    assert camera.acquisition_mode() == "MultiFrame"
    camera.set_acquisition_mode("Continuous")


def test_triggers(camera, fake_gige):
    camera.enable_trigger("FrameStart", "Software")
    assert u32(fake_gige.peek(REGS["TriggerSelector"], 4)) == 0  # FrameStart
    assert u32(fake_gige.peek(REGS["TriggerSource"], 4)) == 1  # Software
    assert u32(fake_gige.peek(REGS["TriggerMode"], 4)) == 1  # On
    fired = fake_gige.software_triggers()
    camera.trigger_software()
    assert fake_gige.software_triggers() == fired + 1
    camera.disable_trigger("FrameStart")
    assert u32(fake_gige.peek(REGS["TriggerMode"], 4)) == 0


def test_region_of_interest_and_binning(camera, fake_gige):
    camera.set_roi(vg.Roi(x=16, y=8, width=320, height=240))
    assert [u32(fake_gige.peek(REGS[n], 4)) for n in ("OffsetX", "OffsetY", "Width", "Height")] == [
        16,
        8,
        320,
        240,
    ]
    assert camera.roi() == vg.Roi(16, 8, 320, 240)
    camera.set_roi(vg.Roi(0, 0, 640, 480))

    camera.set_binning(2, 2)
    assert u32(fake_gige.peek(REGS["BinningHorizontal"], 4)) == 2
    assert u32(fake_gige.peek(REGS["BinningVertical"], 4)) == 2
    assert camera.binning() == (2, 2)
    camera.set_binning(1, 1)
    assert camera.binning() == (1, 1)


def test_frame_rate(camera, fake_gige):
    assert camera.set_frame_rate(12.5) == "AcquisitionFrameRate"
    assert u32(fake_gige.peek(REGS["AcquisitionFrameRateEnable"], 4)) == 1
    assert f32(fake_gige.peek(REGS["AcquisitionFrameRate"], 4)) == 12.5
    assert camera.frame_rate() == 12.5
    camera.disable_frame_rate()
    assert u32(fake_gige.peek(REGS["AcquisitionFrameRateEnable"], 4)) == 0


def test_user_sets(camera, fake_gige):
    assert {"Default", "UserSet1"} <= set(camera.user_sets())
    camera.set_exposure_time(7000.0)
    camera.save_user_set("UserSet1")
    assert fake_gige.saved_exposure(1) == 7000.0
    camera.set_exposure_time(9000.0)
    camera.load_user_set("UserSet1")
    assert f64(fake_gige.peek(REGS["ExposureTime"], 8)) == 7000.0
    assert camera.exposure_time() == 7000.0
    assert camera.set_default_user_set("UserSet1") == "UserSetDefault"
    assert u32(fake_gige.peek(REGS["UserSetDefault"], 4)) == 1
    camera.set_default_user_set("Default")
    camera.load_user_set("Default")


def test_is_done_after_a_command(camera):
    camera.execute("TriggerSoftware", wait=False)
    assert camera.is_done("TriggerSoftware") is True


# ── registers, IP configuration, action commands ───────────────────────────


def test_raw_registers(camera, fake_gige):
    ccp = camera.read_register(REGS["CCP"])
    assert ccp == fake_gige.peek(REGS["CCP"], 4)
    assert u32(ccp) & 0x2, "this application holds control"

    camera.get("Width")  # cached
    camera.write_register(REGS["Width"], (320).to_bytes(4, "big"))
    assert u32(fake_gige.peek(REGS["Width"], 4)) == 320
    assert camera.get("Width") == "320", "a raw write forgets cached values"
    camera.write_register(REGS["Width"], (640).to_bytes(4, "big"))


def test_persistent_ip(camera, fake_gige):
    camera.set_persistent_ip("192.168.7.20", "255.255.255.0", "192.168.7.1")
    assert fake_gige.peek(REGS["PersistentIPAddress"], 4) == socket.inet_aton("192.168.7.20")
    assert fake_gige.peek(REGS["PersistentSubnetMask"], 4) == socket.inet_aton("255.255.255.0")
    assert fake_gige.peek(REGS["PersistentDefaultGateway"], 4) == socket.inet_aton("192.168.7.1")
    assert u32(fake_gige.peek(REGS["CurrentIPConfiguration"], 4)) & 0x2, "persistent IP on"


def test_action_commands_reach_the_cameras_they_address(fake_gige):
    before = fake_gige.actions()
    summary = vg.send_action(
        fake_gige.DEVICE_KEY,
        fake_gige.GROUP_KEY,
        fake_gige.GROUP_MASK,
        destination=fake_gige.ip,
        timeout_ms=1000,
    )
    assert summary == vg.AckSummary(sent=1, acks=1)
    assert fake_gige.actions() == before + 1

    other = vg.send_action(
        fake_gige.DEVICE_KEY,
        fake_gige.GROUP_KEY ^ 0xFFFF,
        fake_gige.GROUP_MASK,
        destination=fake_gige.ip,
        timeout_ms=300,
    )
    assert other.acks == 0
    assert fake_gige.actions() == before + 1, "a camera of another group does not act"


def test_force_ip_refuses_a_malformed_mac_before_sending():
    with pytest.raises(vg.ParseError, match="MAC"):
        vg.force_ip("DE:AD:BE:EF", "10.0.0.2", "255.0.0.0")


# ── closing ────────────────────────────────────────────────────────────────


def test_closing_gives_control_back(fake_gige):
    info = fake_gige.device_info()
    with vg.connect_gige(info) as cam:
        assert fake_gige.controller() is not None
        assert not cam.closed
    assert fake_gige.controller() is None, "the camera holds no control after close"
    assert cam.closed
    with pytest.raises(vg.GenicamError, match="closed"):
        cam.get("Width")
    cam.close()  # again: nothing to do
    with vg.connect_gige(info) as again:
        assert fake_gige.controller() is not None
        assert again.get_integer("Width") > 0


# ── frames, chunks, events, counters, time ─────────────────────────────────


def test_frames_carry_their_block_and_status(camera, fake_gige):
    with camera.stream(packet_size=1500) as frames:
        first = frames.next_frame(timeout_ms=5000)
        second = frames.next_frame(timeout_ms=5000)
        stats = frames.stats()
    assert first.status is vg.FrameStatus.COMPLETE and first.is_complete
    assert first.missing_packets is None
    assert first.payload_type == 0x0001
    assert second.block_id > first.block_id >= 1
    assert isinstance(stats, vg.StreamStats)
    assert stats.frames_complete >= 2
    assert stats.frames_complete <= fake_gige.blocks_sent()
    assert stats.packets_received > 0
    assert stats.frames_incomplete == 0


def test_chunks_read_through_the_cameras_xml(camera, fake_gige):
    camera.configure_chunks(["FrameID", "Timestamp"])
    assert u32(fake_gige.peek(REGS["ChunkModeActive"], 4)) == 1
    try:
        with camera.stream(packet_size=1500) as frames:
            got = frames.next_frame(timeout_ms=5000)
    finally:
        camera.configure_chunks([], active=False)
    assert got.payload_type == 0x4001, "an image with chunks"
    assert {CHUNK_TIMESTAMP, CHUNK_FRAME_ID} <= set(got.chunks)
    values = camera.chunk_features(got)
    assert values["ChunkFrameID"] == str(got.block_id)
    assert values["ChunkTimestamp"] == str(got.ts_dev)


def test_events_and_their_features(camera, loopback_iface):
    with camera.events(["ExposureEnd"], iface=loopback_iface) as events:
        assert events.port > 0
        with camera.stream(packet_size=1500):
            event = events.next_event(timeout_ms=5000)
    assert event.id == EVENT_EXPOSURE_END
    values = camera.event_features(event)
    # The fake timestamps the event like the block's leader and puts the
    # block id in the data.
    assert values["EventExposureEndFrameID"] == str(event.block_id)
    assert values["EventExposureEndTimestamp"] == str(event.ts_dev)


def test_map_dev_ts_follows_time_calibrate(camera):
    assert camera.map_dev_ts(123_456) is None, "nothing to map with before a fit"
    camera.time_calibrate(samples=4, interval_ms=10)
    with camera.stream(packet_size=1500) as frames:
        got = frames.next_frame(timeout_ms=5000)
    assert got.ts_host is not None
    assert abs(camera.map_dev_ts(got.ts_dev) - got.ts_host) < 1e-3
