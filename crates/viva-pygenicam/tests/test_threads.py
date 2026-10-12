"""Cameras and frames from any thread (PY-01); calls that wait on the camera let
other threads run (PY-04).

Each test starts its own fake: one of them delays its answers, which the
module-wide fixture must not.
"""

from __future__ import annotations

import threading
import time

import viva_genicam as vg
from viva_genicam.testing import FakeGigeCamera

REGS = FakeGigeCamera.REGISTERS


def u32(data: bytes) -> int:
    return int.from_bytes(data, "big")


def run_in_thread(target) -> list[BaseException]:
    """Run `target` on a new thread and return what it raised. A PyO3 panic
    is a `BaseException`, so catch that."""
    errors: list[BaseException] = []

    def body() -> None:
        try:
            target()
        except BaseException as err:  # noqa: BLE001
            errors.append(err)

    thread = threading.Thread(target=body)
    thread.start()
    thread.join(60)
    assert not thread.is_alive(), "the thread hung"
    return errors


def test_a_camera_and_a_frame_work_from_another_thread():
    with FakeGigeCamera(width=320, height=240, fps=20) as fake:
        cam = vg.connect_gige(fake.device_info())
        with cam.stream(packet_size=1500) as frames:
            got = frames.next_frame(timeout_ms=5000)
        assert got is not None

        seen: dict = {}

        def use_them() -> None:
            cam.set("Width", "160")
            seen["width"] = cam.get("Width")
            seen["frame"] = (got.width, got.height, got.to_numpy().shape)

        errors = run_in_thread(use_them)
        assert errors == []
        assert u32(fake.peek(REGS["Width"], 4)) == 160, "the write reached the camera"
        assert seen == {"width": "160", "frame": (320, 240, (240, 320))}
        del cam


def test_threads_share_one_camera():
    """Four threads reading and writing one camera at once see the camera's
    values: the calls are serialised, none is lost."""
    with FakeGigeCamera(width=320, height=240) as fake:
        cam = vg.connect_gige(fake.device_info())
        barrier = threading.Barrier(4)
        readings: list[str] = []

        errors: list[BaseException] = []

        def worker(offset: int) -> None:
            try:
                barrier.wait()
                for _ in range(10):
                    cam.set("OffsetX", str(offset * 8))
                    readings.append(cam.get("Height"))
            except BaseException as err:  # noqa: BLE001
                errors.append(err)

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(60)
        assert errors == []
        assert readings == ["240"] * 40
        assert u32(fake.peek(REGS["OffsetX"], 4)) in {0, 8, 16, 24}
        del cam


def long_pauses_during(call, delay: float) -> tuple[float, list[float]]:
    """Run `call` while another thread spins, and return how long the call
    took and every pause of the spinning thread longer than `delay / 2` that
    overlapped it. A call holding the GIL stops the spinner for its whole
    length; one that releases it does not stop it at all."""
    stop = threading.Event()
    pauses: list[tuple[float, float]] = []

    def spin() -> None:
        last = time.monotonic()
        while not stop.is_set():
            now = time.monotonic()
            if now - last > delay / 2:
                pauses.append((last, now))
            last = now

    spinner = threading.Thread(target=spin)
    spinner.start()
    time.sleep(0.05)
    started = time.monotonic()
    call()
    ended = time.monotonic()
    time.sleep(0.05)
    stop.set()
    spinner.join()
    overlapping = [b - a for a, b in pauses if a < ended and b > started]
    return ended - started, overlapping


def test_node_info_lets_other_threads_run():
    """`node_info` reads the camera — `ExposureTime`'s effective access reads
    `ExposureAuto`, its `pIsLocked` — and the fake takes 0.4 s to answer that
    read. Other threads must run meanwhile."""
    delay = 0.4
    with FakeGigeCamera(delay_ack={REGS["ExposureAuto"]: delay}) as fake:
        cam = vg.connect_gige(fake.device_info())
        took, pauses = long_pauses_during(lambda: cam.node_info("ExposureTime"), delay)
        assert took >= delay * 0.9, f"the fake's delay applied ({took:.2f} s)"
        assert pauses == [], f"another thread stopped for {pauses} s"
        del cam


def test_feature_state_lets_other_threads_run():
    """The same for a call new in this release: `feature_state` reads the
    feature's lock, through the same delayed register."""
    delay = 0.4
    with FakeGigeCamera(delay_ack={REGS["ExposureAuto"]: delay}) as fake:
        cam = vg.connect_gige(fake.device_info())
        took, pauses = long_pauses_during(lambda: cam.feature_state("ExposureTime"), delay)
        assert took >= delay * 0.9, f"the fake's delay applied ({took:.2f} s)"
        assert pauses == [], f"another thread stopped for {pauses} s"
        cam.close()
