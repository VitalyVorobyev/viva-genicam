import type { JsonValue } from "../../device/types";

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
