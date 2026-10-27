import type { SfncGroup } from "../../device/types";

export function isSectionApplicable(
  group: SfncGroup,
  nodesById: Record<string, unknown>
): boolean {
  if (Object.keys(nodesById).length === 0) return true;
  if (group.features.length === 0) return false;
  return group.features.some((f) => [f.node, ...(f.aliases ?? [])].some((name) => name in nodesById));
}
