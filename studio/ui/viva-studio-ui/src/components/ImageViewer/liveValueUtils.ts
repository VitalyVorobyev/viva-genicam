import type { NodeValueEntry } from "../../device/types";

/**
 * Extract a numeric value from the live values map.
 * Handles both JSON number (640) and string ("640") forms,
 * since the camera service may publish values as strings.
 */
export function getLiveNumber(
  liveValues: Map<string, NodeValueEntry>,
  name: string,
): number | null {
  const entry = liveValues.get(name);
  if (entry === undefined) return null;
  if (typeof entry.value === "number") return entry.value;
  if (typeof entry.value === "string") {
    const n = parseFloat(entry.value);
    return isNaN(n) ? null : n;
  }
  return null;
}

/** Extract a string value from the live values map. */
export function getLiveString(
  liveValues: Map<string, NodeValueEntry>,
  name: string,
): string | null {
  const entry = liveValues.get(name);
  if (entry === undefined || entry.value === null) return null;
  return String(entry.value);
}
