import { useMemo } from "react";
import { useFeature } from "../../device/FeatureStoreContext";
import type { UiGraph } from "../../xml_model/uigraph";
import type { VisibilityFilter } from "./FeatureBrowserPage";
import {
  isUnknownKind,
  nodeDisplayName,
  nodeKindCssKey,
  nodeKindIcon,
} from "../../xml_model/helpers";
import { visibilityPassesFilter } from "./FeatureBrowserPage";
import { formatLiveValue } from "./treeUtils";

interface FeatureListProps {
  graph: UiGraph | null;
  categoryName: string | null;
  visibilityFilter: VisibilityFilter;
  selectedNodeName: string | null;
  onSelectNode: (name: string) => void;
  onSelectCategory: (name: string) => void;
}

interface FeatureRow {
  name: string;
  displayName: string;
  kindKey: string;
  icon: string;
  declaredAccess: string | null;
  isCategory: boolean;
  subFeatureCount?: number;
}

/** Middle column: features within the selected category. */
export function FeatureList({
  graph,
  categoryName,
  visibilityFilter,
  selectedNodeName,
  onSelectNode,
  onSelectCategory,
}: FeatureListProps) {
  const { categoryTitle, features } = useMemo(() => {
    if (!graph || !categoryName) return { categoryTitle: "", features: [] as FeatureRow[] };
    const cat = graph.categories[categoryName];
    if (!cat) return { categoryTitle: categoryName, features: [] as FeatureRow[] };

    const rows: FeatureRow[] = [];
    for (const featureName of cat.features) {
      const node = graph.nodes_by_name[featureName];
      if (!node) continue;

      const isCategory = node.kind === "Category";

      if (!isCategory) {
        if (isUnknownKind(node.kind)) continue;
        if (!visibilityPassesFilter(node.visibility, visibilityFilter)) continue;
      }

      const subCat = graph.categories[featureName];
      const subFeatureCount = subCat ? subCat.features.length : undefined;

      rows.push({
        name: featureName,
        displayName: nodeDisplayName(node),
        kindKey: isCategory ? "category" : nodeKindCssKey(node.kind),
        icon: isCategory ? "\u25A6" : nodeKindIcon(node.kind),
        declaredAccess: node.access_mode ?? null,
        isCategory,
        subFeatureCount,
      });
    }
    return { categoryTitle: cat.display_name, features: rows };
  }, [graph, categoryName, visibilityFilter]);

  if (!graph) {
    return (
      <div className="feat-list feat-list--empty">
        <span className="feat-list__hint">No model loaded</span>
      </div>
    );
  }

  if (!categoryName) {
    return (
      <div className="feat-list feat-list--empty">
        <span className="feat-list__hint">Select a category</span>
      </div>
    );
  }

  return (
    <div className="feat-list" role="listbox" aria-label={categoryTitle}>
      <div className="feat-list__header">{categoryTitle}</div>
      <div className="feat-list__scroll">
        {features.length === 0 ? (
          <div className="feat-list__hint">No visible features</div>
        ) : (
          features.map((feat) => (
            <FeatureListItem
              key={feat.name}
              feat={feat}
              isActive={feat.name === selectedNodeName}
              onSelect={() =>
                feat.isCategory ? onSelectCategory(feat.name) : onSelectNode(feat.name)
              }
            />
          ))
        )}
      </div>
    </div>
  );
}

/**
 * One row. It follows its feature's live state itself, so the list reads from
 * the camera only the features it shows, and a change re-renders one row.
 */
function FeatureListItem({
  feat,
  isActive,
  onSelect,
}: {
  feat: FeatureRow;
  isActive: boolean;
  onSelect: () => void;
}) {
  const state = useFeature(feat.isCategory ? null : feat.name);
  const liveText = state ? formatLiveValue(state) : null;
  const access = state?.access_mode ?? feat.declaredAccess;
  return (
    <button
      type="button"
      role="option"
      aria-selected={isActive}
      className={`feat-list__item${isActive ? " feat-list__item--active" : ""}${feat.isCategory ? " feat-list__item--category" : ""}${state && !state.is_available ? " feat-list__item--unavailable" : ""}`}
      onClick={onSelect}
      title={state && !state.is_available ? `${feat.name} — not available now` : feat.name}
    >
      <span className={`feat-list__icon feat-list__icon--${feat.kindKey}`}>{feat.icon}</span>
      {access === "RO" && <span className="feat-list__ro">RO</span>}
      <span className="feat-list__name">{feat.displayName}</span>
      {liveText !== null ? (
        <span className="feat-list__live">{liveText}</span>
      ) : feat.isCategory && feat.subFeatureCount !== undefined ? (
        <span className="feat-list__meta">{feat.subFeatureCount}</span>
      ) : null}
      {feat.isCategory && <span className="feat-list__chevron">{"\u203A"}</span>}
    </button>
  );
}
