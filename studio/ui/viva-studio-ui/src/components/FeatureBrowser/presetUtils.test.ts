import { describe, it, expect } from "vitest";
import { buildLiveValuePreset, presetValueToDraft } from "./presetUtils";
import type { UiNode } from "../../xml_model/uigraph";
import type { JsonValue } from "../../device/types";

function entry(value: JsonValue) {
  return { value };
}

describe("buildLiveValuePreset", () => {
  it("test_buildLiveValuePreset_empty", () => {
    expect(buildLiveValuePreset(new Map())).toEqual({});
  });

  // A register's state is a summary of its size, not a value to restore.
  it("leaves out a register's size summary", () => {
    const map = new Map([
      ["Width", entry(640)],
      ["FileAccessBuffer", entry({ kind: "register", length: 100000 })],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({ Width: 640 });
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
    const map = new Map([
      ["Width", entry(640)],
      ["AcquisitionStart", entry(null)],
      ["UserSetLoadReg", entry(null)],
    ]);
    expect(buildLiveValuePreset(map)).toEqual({ Width: 640 });
  });
});

function node(kind: UiNode["kind"], access_mode = "RW"): UiNode {
  return {
    name: "N",
    kind,
    access_mode,
  };
}

// "Export State" writes the camera's live values, where an enumeration is its
// entry name; "Export Preset" writes drafts, where it is `{ enumName }`. Both
// must import as drafts the editors and the validator accept.
describe("presetValueToDraft", () => {
  it("turns an exported enum entry name into an enum draft", () => {
    expect(presetValueToDraft(node("Enumeration"), "Mono8")).toEqual({ enumName: "Mono8" });
  });

  it("keeps an enum draft exported as a draft", () => {
    expect(presetValueToDraft(node("Enumeration"), { enumName: "Mono8" })).toEqual({
      enumName: "Mono8",
    });
  });

  it("keeps a value of the node's own type", () => {
    expect(presetValueToDraft(node("Integer"), 640)).toBe(640);
    expect(presetValueToDraft(node("Float"), 1.5)).toBe(1.5);
    expect(presetValueToDraft(node("Boolean"), true)).toBe(true);
    expect(presetValueToDraft(node("String"), "cam-1")).toBe("cam-1");
  });

  it("leaves out a value of the wrong type", () => {
    expect(presetValueToDraft(node("Integer"), "640")).toBeUndefined();
    expect(presetValueToDraft(node("Boolean"), 1)).toBeUndefined();
  });

  it("leaves out what cannot be written", () => {
    expect(presetValueToDraft(node("Integer", "RO"), 640)).toBeUndefined();
    expect(presetValueToDraft(node("Integer", "NA"), 640)).toBeUndefined();
    expect(presetValueToDraft(node("Command", "WO"), true)).toBeUndefined();
  });
});
