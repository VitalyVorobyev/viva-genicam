import { describe, it, expect } from "vitest";
import type { SfncGroup } from "../../device/types";
import { isSectionApplicable } from "./sfncGroupUtils";

function makeGroup(features: { node: string }[]): SfncGroup {
  return {
    id: "test_group",
    title: "Test Group",
    icon: "sun",
    default_open: false,
    features: features.map((f) => ({ node: f.node, widget: "float_input" })),
  };
}

describe("isSectionApplicable", () => {
  it("test_isSectionApplicable_empty_nodesById_returns_true", () => {
    const group = makeGroup([{ node: "ExposureTime" }]);
    expect(isSectionApplicable(group, {})).toBe(true);
  });

  it("test_isSectionApplicable_matching_node_returns_true", () => {
    const group = makeGroup([{ node: "ExposureTime" }]);
    const nodes = { ExposureTime: {} };
    expect(isSectionApplicable(group, nodes)).toBe(true);
  });

  it("test_isSectionApplicable_no_matching_node_returns_false", () => {
    const group = makeGroup([{ node: "ExposureTime" }]);
    const nodes = { Gain: {} };
    expect(isSectionApplicable(group, nodes)).toBe(false);
  });

  it("test_isSectionApplicable_partial_match_returns_true", () => {
    const group = makeGroup([
      { node: "ExposureTime" },
      { node: "ExposureAuto" },
      { node: "Gain" },
    ]);
    const nodes = { Gain: {} };
    expect(isSectionApplicable(group, nodes)).toBe(true);
  });

  it("test_isSectionApplicable_empty_features_returns_false", () => {
    const group = makeGroup([]);
    const nodes = { ExposureTime: {} };
    expect(isSectionApplicable(group, nodes)).toBe(false);
  });

  it("test_isSectionApplicable_empty_features_empty_nodes_returns_true", () => {
    const group = makeGroup([]);
    expect(isSectionApplicable(group, {})).toBe(true);
  });
});
