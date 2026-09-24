import { describe, it, expect } from "vitest";
import { buildLiveValuePreset } from "./presetUtils";
import type { NodeValueEntry } from "../../device/types";

function entry(value: number | string | boolean): NodeValueEntry {
  return { value, access_mode: "RW" };
}

describe("buildLiveValuePreset", () => {
  it("test_buildLiveValuePreset_empty", () => {
    expect(buildLiveValuePreset(new Map())).toEqual({});
  });

  it("test_buildLiveValuePreset_numbers", () => {
    const map = new Map([
      ["Width", entry(640)],
      ["Height", entry(480)],
      ["Gain", entry(1.5)],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({
      Width: 640,
      Height: 480,
      Gain: 1.5,
    });
  });

  it("test_buildLiveValuePreset_strings", () => {
    const map = new Map([
      ["PixelFormat", entry("Mono8")],
      ["AcquisitionMode", entry("Continuous")],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({
      PixelFormat: "Mono8",
      AcquisitionMode: "Continuous",
    });
  });

  it("test_buildLiveValuePreset_booleans", () => {
    const map = new Map([
      ["GainAuto", entry(false)],
      ["ExposureAuto", entry(true)],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({
      GainAuto: false,
      ExposureAuto: true,
    });
  });

  it("test_buildLiveValuePreset_mixed", () => {
    const map = new Map([
      ["Width", entry(640)],
      ["PixelFormat", entry("Mono8")],
      ["GainAuto", entry(false)],
    ]);
    const result = buildLiveValuePreset(map);
    expect(result["Width"]).toBe(640);
    expect(result["PixelFormat"]).toBe("Mono8");
    expect(result["GainAuto"]).toBe(false);
    expect(Object.keys(result)).toHaveLength(3);
  });

  it("test_buildLiveValuePreset_access_mode_ignored", () => {
    // access_mode is not included in the output; only the value is exported.
    const map = new Map([["ExposureTime", { value: 5000, access_mode: "RO" }]]);
    const result = buildLiveValuePreset(map);
    expect(result).toEqual({ ExposureTime: 5000 });
    // The result is a plain value record with no extra fields.
    expect(Object.keys(result)).toEqual(["ExposureTime"]);
  });

  it("test_buildLiveValuePreset_skips_features_without_a_value", () => {
    // Commands and write-only nodes carry `null`: nothing to restore.
    const map = new Map<string, NodeValueEntry>([
      ["Width", entry(640)],
      ["AcquisitionStart", { value: null, access_mode: "WO" }],
      ["UserSetLoadReg", { value: null, access_mode: "WO" }],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({ Width: 640 });
  });
});
