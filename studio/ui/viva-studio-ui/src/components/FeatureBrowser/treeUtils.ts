import type { JsonValue } from "../../device/types";
import type { UiGraph } from "../../xml_model/uigraph";

/**
 * The toolbar's one-line description of a model. Declarations the library
 * could not turn into features are counted, so a missing feature reads as
 * dropped rather than absent (backlog DX-05); the Diagnostics tab lists them.
 */
export function graphSummary(graph: UiGraph, nodeCount: number, categoryCount: number): string {
  const summary = `${nodeCount} nodes · ${categoryCount} cat`;
  const skipped = graph.skipped?.length ?? 0;
  return skipped > 0 ? `${summary} · ${skipped} not shown` : summary;
}

/**
 * Format a live node value for compact inline display in the category tree.
 *
 * Rules:
 * - boolean → "true" / "false"
 * - integer (Number.isInteger) → decimal string, no fraction
 * - float → 3-decimal fixed, trailing zeros stripped
 * - string → returned as-is
 * - null (a command or write-only node: nothing to read) → null, so the tree
 *   shows no value rather than the text "null"
 * - a register's `{ kind, length }` summary → its size in bytes
 */
export function formatLiveValue(state: { value: JsonValue }): string | null {
  const { value } = state;
  if (value === null) return null;
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") {
    if (Number.isInteger(value)) return String(value);
    return value.toFixed(3).replace(/\.?0+$/, "");
  }
  if (typeof value === "string") return value;
  // A `Register` reports its size, not its bytes.
  if (!Array.isArray(value) && typeof value.length === "number") return `${value.length} B`;
  return null;
}

/**
 * The categories from the root down to `category`, both included; empty when
 * the root does not reach it. Breadth-first, so a category listed under two
 * parents takes the shorter path, and a cycle in a malformed document ends.
 */
export function categoryPath(graph: UiGraph, category: string): string[] {
  const parent = new Map<string, string | null>([[graph.root_category, null]]);
  const queue = [graph.root_category];
  while (queue.length > 0) {
    const current = queue.shift()!;
    if (current === category) {
      const path = [current];
      for (let up = parent.get(current); up != null; up = parent.get(up)) path.unshift(up);
      return path;
    }
    for (const child of graph.categories[current]?.features ?? []) {
      if (graph.categories[child] && !parent.has(child)) {
        parent.set(child, current);
        queue.push(child);
      }
    }
  }
  return [];
}
