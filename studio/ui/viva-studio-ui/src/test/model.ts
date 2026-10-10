import type { ParseXmlResponse, UiCategory, UiNode } from "../xml_model/uigraph";

/**
 * Build a `ParseXmlResponse` from partial nodes, the way the Tauri
 * `parse_xml` command would return it. Every node defaults to a writable
 * Integer; a category `Root` lists every node unless `categories` says
 * otherwise.
 */
export function makeModel(
  nodes: Record<string, Partial<UiNode>>,
  categories?: Record<string, UiCategory>,
): ParseXmlResponse {
  const nodes_by_name: Record<string, UiNode> = {};
  for (const [name, partial] of Object.entries(nodes)) {
    nodes_by_name[name] = {
      name,
      kind: "Integer",
      access_mode: "RW",
      enum_entries: [],
      ...partial,
    };
  }
  const cats = categories ?? {
    Root: { name: "Root", display_name: "Root", features: Object.keys(nodes) },
  };
  for (const name of Object.keys(cats)) {
    nodes_by_name[name] ??= {
      name,
      kind: "Category",
      enum_entries: [],
    };
  }
  return {
    graph: { nodes_by_name, categories: cats, root_category: "Root" },
    xml: "",
    diags: [],
    summary: {
      node_count: Object.keys(nodes_by_name).length,
      category_count: Object.keys(cats).length,
      root_category: "Root",
    },
  };
}
