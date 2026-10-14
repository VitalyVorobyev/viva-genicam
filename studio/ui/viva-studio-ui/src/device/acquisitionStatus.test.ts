import { describe, expect, it } from "vitest";
import {
  NO_ACQUISITION,
  describeAcquisition,
  followAcquisition,
  streamTransport,
} from "./acquisitionStatus";
import type { AcquisitionStatus, DeviceInfo, StreamCounters } from "./types";
import { counters } from "../test/streamCounters";

describe("describeAcquisition", () => {
  it("says it is acquiring before the first rate arrives", () => {
    expect(describeAcquisition({ active: true, fps: null, stats: null })).toEqual({
      text: "Acquiring",
      dropping: false,
      title: undefined,
    });
  });

  it("shows the rate frames arrive from the camera", () => {
    const status = { active: true, fps: 240.96, stats: counters({ frames_complete: 2410 }) };
    expect(describeAcquisition(status)).toEqual({
      text: "241.0 fps",
      dropping: false,
      title: "Frames received from the camera. Lost: incomplete 0, missed 0, dropped by host 0",
    });
  });

  // A frame lost between camera and app is the symptom of a link problem
  // such as #112's, and was invisible (backlog ST-36). Lost is the library's
  // `frames_lost`: incomplete, missed and dropped by the host.
  it("counts lost frames once there are any, and says why in the tooltip", () => {
    const stats = counters({ frames_incomplete: 1, frames_missed: 2, frames_backpressure: 4 });
    expect(describeAcquisition({ active: true, fps: 12.5, stats })).toEqual({
      text: "12.5 fps · 7 lost",
      dropping: true,
      title: "Frames received from the camera. Lost: incomplete 1, missed 2, dropped by host 4",
    });
  });
});

describe("followAcquisition", () => {
  const status = (active: boolean, stats: StreamCounters | null): AcquisitionStatus => ({
    active,
    fps: active ? 30 : null,
    stats,
  });

  it("keeps a stopped stream's counters through the status that follows them", () => {
    const final = counters({ frames_complete: 900 });
    let state = followAcquisition(NO_ACQUISITION, status(true, null));
    state = followAcquisition(state, status(true, counters({ frames_complete: 30 })));
    state = followAcquisition(state, status(false, final));
    state = followAcquisition(state, status(false, null));
    expect(state.status.active).toBe(false);
    expect(state.stats).toBe(final);
  });

  it("starts a new count when acquisition starts", () => {
    let state = followAcquisition(NO_ACQUISITION, status(false, counters({ frames_complete: 900 })));
    state = followAcquisition(state, status(true, null));
    expect(state.stats).toBeNull();
    // A second start status — the service's and the app's — keeps nothing old.
    state = followAcquisition(state, status(true, null));
    expect(state.stats).toBeNull();
  });
});

describe("streamTransport", () => {
  const device = (id: string, transport: string): DeviceInfo => ({
    id,
    name: "cam",
    model: "M",
    serial: "S",
    transport,
  });

  it("follows the device list, and a remote device's id", () => {
    expect(streamTransport(device("cam-0011", "gige"), "cam-0011")).toBe("gige");
    expect(streamTransport(device("u3v-164c5533-1", "usb3"), "u3v-164c5533-1")).toBe("u3v");
    expect(streamTransport(device("cam-u3v-164c5533-FF00", "zenoh"), "cam-u3v-164c5533-FF00")).toBe(
      "u3v",
    );
    expect(streamTransport(device("cam-0011", "zenoh"), "cam-0011")).toBe("gige");
    expect(streamTransport(undefined, "u3v-164c5533-1")).toBe("u3v");
  });
});
