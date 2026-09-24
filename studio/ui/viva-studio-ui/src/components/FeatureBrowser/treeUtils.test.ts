import { describe, it, expect } from "vitest";
import { formatLiveValue } from "./treeUtils";
import type { NodeValueEntry } from "../../device/types";

function entry(value: number | string | boolean): NodeValueEntry {
  return { value, access_mode: "RW" };
}

describe("formatLiveValue", () => {
  it("test_formatLiveValue_number_integer", () => {
    expect(formatLiveValue(entry(42))).toBe("42");
  });

  it("test_formatLiveValue_number_integer_zero", () => {
    expect(formatLiveValue(entry(0))).toBe("0");
  });

  it("test_formatLiveValue_number_float", () => {
    expect(formatLiveValue(entry(3.14159))).toBe("3.142");
  });

  it("test_formatLiveValue_number_float_trailing_zeros", () => {
    // 2.500 → "2.5" (trailing zeros stripped)
    expect(formatLiveValue(entry(2.5))).toBe("2.5");
  });

  it("test_formatLiveValue_number_float_whole_after_strip", () => {
    // e.g. 1.000 → "1" (trailing .000 fully stripped)
    expect(formatLiveValue(entry(1.0))).toBe("1");
  });

  it("test_formatLiveValue_string", () => {
    expect(formatLiveValue(entry("Continuous"))).toBe("Continuous");
  });

  it("test_formatLiveValue_string_empty", () => {
    expect(formatLiveValue(entry(""))).toBe("");
  });

  it("test_formatLiveValue_boolean_true", () => {
    expect(formatLiveValue(entry(true))).toBe("true");
  });

  it("test_formatLiveValue_boolean_false", () => {
    expect(formatLiveValue(entry(false))).toBe("false");
  });

  it("test_formatLiveValue_null_is_no_value", () => {
    // A write-only or command node: nothing to show, not the text "null".
    expect(formatLiveValue({ value: null, access_mode: "WO" })).toBeNull();
  });
});
