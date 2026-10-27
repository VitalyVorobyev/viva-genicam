import { memo } from "react";
import type { SfncGroup } from "../../device/types";
import type { UiNode } from "../../xml_model/uigraph";
import { SfncFeatureControl } from "./SfncFeatureControl";

interface FeatureGroupSectionProps {
  group: SfncGroup;
  nodes: Record<string, UiNode>;
  isConnected: boolean;
}

/**
 * The features of one `sfnc-groups.json` group that this camera has, each as
 * a {@link FeatureControl} with the widget the group names. Memoised so the
 * viewer's per-frame re-render does not reach it; each control re-renders
 * only when its own feature changes.
 */
export const FeatureGroupSection = memo(function FeatureGroupSection({
  group,
  nodes,
  isConnected,
}: FeatureGroupSectionProps) {
  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }
  return (
    <div className="feature-group">
      {group.features
        .filter((f) => [f.node, ...(f.aliases ?? [])].some((name) => nodes[name]))
        .map((f) => (
          <SfncFeatureControl key={f.node} feature={f} nodes={nodes} />
        ))}
    </div>
  );
});
