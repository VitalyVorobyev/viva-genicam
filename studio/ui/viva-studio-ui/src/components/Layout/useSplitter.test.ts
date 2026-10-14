import { describe, it, expect } from "vitest";
import { clampSplitter, splitterKeySize } from "./useSplitter";

describe("clampSplitter", () => {
  it("test_clampSplitter_within_bounds", () => {
    expect(clampSplitter(300, 100, 600)).toBe(300);
  });

  it("test_clampSplitter_below_min", () => {
    expect(clampSplitter(50, 100, 600)).toBe(100);
  });

  it("test_clampSplitter_above_max", () => {
    expect(clampSplitter(700, 100, 600)).toBe(600);
  });

  it("test_clampSplitter_at_min", () => {
    expect(clampSplitter(100, 100, 600)).toBe(100);
  });

  it("test_clampSplitter_at_max", () => {
    expect(clampSplitter(600, 100, 600)).toBe(600);
  });

  it("test_clampSplitter_negative_value_clamped_to_min", () => {
    expect(clampSplitter(-50, 160, 420)).toBe(160);
  });
});

// The splitters took focus and ignored every key, so the panes could only be
// resized with a pointer (backlog ST-40).
describe("splitterKeySize", () => {
  const bounds = { minSize: 100, maxSize: 400 };

  it("moves a left/right splitter with the horizontal arrows", () => {
    expect(splitterKeySize("ArrowRight", 200, "horizontal", bounds)).toBe(216);
    expect(splitterKeySize("ArrowLeft", 200, "horizontal", bounds)).toBe(184);
    expect(splitterKeySize("ArrowUp", 200, "horizontal", bounds)).toBeNull();
  });

  it("grows a bottom pane with ArrowUp", () => {
    expect(splitterKeySize("ArrowUp", 200, "vertical", bounds)).toBe(216);
    expect(splitterKeySize("ArrowDown", 200, "vertical", bounds)).toBe(184);
  });

  it("jumps to the bounds with Home and End, and stays within them", () => {
    expect(splitterKeySize("Home", 200, "horizontal", bounds)).toBe(100);
    expect(splitterKeySize("End", 200, "horizontal", bounds)).toBe(400);
    expect(splitterKeySize("ArrowRight", 395, "horizontal", bounds)).toBe(400);
  });

  it("grows a pane right of its handle with ArrowLeft", () => {
    expect(splitterKeySize("ArrowLeft", 200, "horizontal", bounds, true)).toBe(216);
    expect(splitterKeySize("ArrowRight", 200, "horizontal", bounds, true)).toBe(184);
  });

  it("ignores other keys", () => {
    expect(splitterKeySize("a", 200, "horizontal", bounds)).toBeNull();
  });
});
