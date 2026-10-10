import { describe, it, expect } from "vitest";
import { flattenVisibleTree, type FlatRow } from "./flattenTree";
import type { UiGraph, UiCategory, UiNode } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";

function makeGraph(categories: Record<string, UiCategory>, nodes: Record<string, Partial<UiNode>>): UiGraph {
  const fullNodes: Record<string, UiNode> = {};
  for (const [name, partial] of Object.entries(nodes)) {
    fullNodes[name] = {
      name,
      kind: "Integer",
      raw: { tag: "Integer", attributes: {}, children_text: {} },
      enum_entries: [],
      ...partial,
    } as UiNode;
  }
  // Add category nodes
  for (const name of Object.keys(categories)) {
    if (!fullNodes[name]) {
      fullNodes[name] = {
        name,
        kind: "Category",
        raw: { tag: "Category", attributes: {}, children_text: {} },
        enum_entries: [],
      } as UiNode;
    }
  }
  return { nodes_by_name: fullNodes, categories, root_category: "Root" };
}

function flatten(overrides: Partial<Parameters<typeof flattenVisibleTree>[0]> = {}): FlatRow[] {
  const graph = overrides.graph ?? makeGraph(
    {
      Root: { name: "Root", display_name: "Root", features: ["Width", "Height", "SubCat"] },
      SubCat: { name: "SubCat", display_name: "Sub Category", features: ["Gain"] },
    },
    {
      Width: { kind: "Integer", display_name: "Width" },
      Height: { kind: "Integer", display_name: "Height" },
      Gain: { kind: "Float", display_name: "Gain" },
    }
  );

  return flattenVisibleTree({
    graph,
    rootCategory: graph.root_category,
    expanded: new Set(["Root"]),
    hideUnknown: false,
    visibilityFilter: "All",
    favorites: new Set(),
    favExpanded: true,
    liveValues: undefined,
    ...overrides,
  });
}

describe("flattenVisibleTree", () => {
  it("produces correct order with root expanded", () => {
    const rows = flatten();
    const names = rows.map((r) => r.name);
    // Root (expanded) → Width, Height, SubCat (collapsed)
    expect(names).toEqual(["Root", "Width", "Height", "SubCat"]);
  });

  it("sets correct depths", () => {
    const rows = flatten();
    expect(rows[0].depth).toBe(0); // Root
    expect(rows[1].depth).toBe(1); // Width
    expect(rows[3].depth).toBe(1); // SubCat
  });

  it("expands subcategories when in expanded set", () => {
    const rows = flatten({ expanded: new Set(["Root", "SubCat"]) });
    const names = rows.map((r) => r.name);
    expect(names).toEqual(["Root", "Width", "Height", "SubCat", "Gain"]);
    expect(rows[4].depth).toBe(2); // Gain under SubCat
  });

  it("collapses root hides all children", () => {
    const rows = flatten({ expanded: new Set() });
    expect(rows.length).toBe(1);
    expect(rows[0].name).toBe("Root");
    expect(rows[0].isExpanded).toBe(false);
  });

  it("filters unknown nodes when hideUnknown is true", () => {
    const graph = makeGraph(
      { Root: { name: "Root", display_name: "Root", features: ["Width", "Mystery"] } },
      {
        Width: { kind: "Integer" },
        Mystery: { kind: { Unknown: { tag: "Foo" } } },
      }
    );
    const rows = flatten({ graph, hideUnknown: true });
    const names = rows.map((r) => r.name);
    expect(names).toContain("Width");
    expect(names).not.toContain("Mystery");
  });

  it("detects cycles", () => {
    const graph = makeGraph(
      {
        Root: { name: "Root", display_name: "Root", features: ["A"] },
        A: { name: "A", display_name: "A", features: ["Root"] }, // cycle!
      },
      {}
    );
    const rows = flatten({ graph, expanded: new Set(["Root", "A"]) });
    const cycleRow = rows.find((r) => r.displayName.includes("Cycle"));
    expect(cycleRow).toBeDefined();
  });

  it("includes favorites section when favorites are present", () => {
    const rows = flatten({ favorites: new Set(["Width"]) });
    expect(rows[0].type).toBe("favorite-header");
    expect(rows[1].type).toBe("favorite");
    expect(rows[1].name).toBe("Width");
    expect(rows[1].isFavorited).toBe(true);
  });

  it("hides favorite children when favExpanded is false", () => {
    const rows = flatten({ favorites: new Set(["Width"]), favExpanded: false });
    expect(rows[0].type).toBe("favorite-header");
    expect(rows[0].isExpanded).toBe(false);
    // Next row should be the Root category, not a favorite leaf
    expect(rows[1].type).toBe("category");
  });

  it("includes live text when liveValues provided", () => {
    const liveValues = new Map<string, NodeValueEntry>([
      ["Width", { value: 640, access_mode: "RW" }],
    ]);
    const rows = flatten({ liveValues });
    const widthRow = rows.find((r) => r.name === "Width" && r.type === "leaf");
    expect(widthRow?.liveText).toBe("640");
  });

  it("sets accessMode from live values", () => {
    const liveValues = new Map<string, NodeValueEntry>([
      ["Width", { value: 640, access_mode: "RO" }],
    ]);
    const rows = flatten({ liveValues });
    const widthRow = rows.find((r) => r.name === "Width" && r.type === "leaf");
    expect(widthRow?.accessMode).toBe("RO");
  });

  it("handles missing node references gracefully", () => {
    const graph = makeGraph(
      { Root: { name: "Root", display_name: "Root", features: ["Width", "NonExistent"] } },
      { Width: { kind: "Integer" } }
    );
    const rows = flatten({ graph });
    const missingRow = rows.find((r) => r.name === "NonExistent");
    expect(missingRow?.type).toBe("missing");
  });
});
