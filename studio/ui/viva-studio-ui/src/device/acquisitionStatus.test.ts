import { describe, expect, it } from "vitest";
import { describeAcquisition } from "./acquisitionStatus";

describe("describeAcquisition", () => {
  it("says it is acquiring before the first rate arrives", () => {
    expect(describeAcquisition({ active: true, fps: null, dropped: 0 })).toEqual({
      text: "Acquiring",
      dropping: false,
    });
  });

  it("shows the camera's frame rate", () => {
    expect(describeAcquisition({ active: true, fps: 29.97, dropped: 0 })).toEqual({
      text: "30.0 fps",
      dropping: false,
    });
  });

  // A frame lost between camera and app is the symptom of a link problem
  // such as #112's, and was invisible (backlog ST-36).
  it("counts dropped frames once there are any", () => {
    expect(describeAcquisition({ active: true, fps: 12.5, dropped: 3 })).toEqual({
      text: "12.5 fps · 3 dropped",
      dropping: true,
    });
  });
});
