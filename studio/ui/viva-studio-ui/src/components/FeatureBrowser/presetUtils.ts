import type { NodeValueEntry } from "../../device/types";
import type { UiNode } from "../../xml_model/uigraph";
import type { NodeValue } from "../../xml_model/values";

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

/**
 * The draft an imported preset entry becomes for `node`, or `undefined` to
 * leave the node out.
 *
 * A preset comes from "Export Preset" (drafts, where an enumeration is
 * `{ enumName }`) or "Export State" (live values, where it is the entry name).
 * Both become the draft the editors and the validator expect. A node that
 * cannot be written, or a value of the wrong type, is left out rather than
 * queued as a write that is bound to fail.
 */
export function presetValueToDraft(node: UiNode, raw: unknown): NodeValue | undefined {
  if (node.access_mode === "RO" || node.access_mode === "NA") return undefined;
  switch (node.kind) {
    case "Enumeration":
      if (typeof raw === "string") return { enumName: raw };
      if (
        typeof raw === "object" &&
        raw !== null &&
        typeof (raw as { enumName?: unknown }).enumName === "string"
      ) {
        return { enumName: (raw as { enumName: string }).enumName };
      }
      return undefined;
    case "Integer":
    case "Float":
      return typeof raw === "number" && Number.isFinite(raw) ? raw : undefined;
    case "Boolean":
      return typeof raw === "boolean" ? raw : undefined;
    case "String":
      return typeof raw === "string" ? raw : undefined;
    default:
      // Commands, registers, categories and unknown kinds hold no settable value.
      return undefined;
  }
}
