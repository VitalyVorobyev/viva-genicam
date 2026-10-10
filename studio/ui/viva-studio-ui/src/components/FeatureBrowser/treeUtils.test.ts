import { describe, it, expect } from "vitest";
import { formatLiveValue, graphSummary } from "./treeUtils";
import type { UiGraph } from "../../xml_model/uigraph";
function entry(value: number | string | boolean) {
  return { value };
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
    expect(formatLiveValue({ value: null })).toBeNull();
  });

  it("shows a register by its size", () => {
    expect(formatLiveValue({ value: { kind: "register", length: 4096 } })).toBe("4096 B");
  });
});

describe("graphSummary", () => {
  const graph = (skipped?: UiGraph["skipped"]): UiGraph => ({
    nodes_by_name: {},
    categories: {},
    root_category: "Root",
    skipped,
  });

  it("counts nodes and categories", () => {
    expect(graphSummary(graph(), 12, 3)).toBe("12 nodes · 3 cat");
  });

  // A feature the library dropped is called out rather than silently absent
  // (backlog DX-05).
  it("says how many declarations could not be shown", () => {
    const skipped = [
      { tag: "Register", name: "LUTValueAll", error: "missing <Length>" },
      { tag: "Integer", error: "no Name" },
    ];
    expect(graphSummary(graph(skipped), 12, 3)).toBe("12 nodes · 3 cat · 2 not shown");
  });
});
