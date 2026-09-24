import type { NodeValueEntry } from "../../device/types";

/**
 * Convert a live-values map to a flat preset record.
 *
 * The result is in the same format as the draft-preset export so it can be
 * re-imported with "Import Preset" and applied with "Apply All" to restore
 * the device state on a subsequent session.
 *
 * Features without a value (commands, write-only nodes) are left out: there
 * is no state to restore.
 *
 * @param liveValues  Current live node values from the Tauri backend.
 * @returns           A plain object mapping node names to their scalar values.
 */
export function buildLiveValuePreset(
  liveValues: Map<string, NodeValueEntry>,
): Record<string, number | string | boolean> {
  const result: Record<string, number | string | boolean> = {};
  for (const [name, entry] of liveValues) {
    if (entry.value === null) continue;
    result[name] = entry.value;
  }
  return result;
}
