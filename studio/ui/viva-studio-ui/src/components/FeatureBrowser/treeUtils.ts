import type { NodeValueEntry } from "../../device/types";

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
 */
export function formatLiveValue(entry: NodeValueEntry): string | null {
  const { value } = entry;
  if (value === null) return null;
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") {
    if (Number.isInteger(value)) return String(value);
    return value.toFixed(3).replace(/\.?0+$/, "");
  }
  return String(value);
}
