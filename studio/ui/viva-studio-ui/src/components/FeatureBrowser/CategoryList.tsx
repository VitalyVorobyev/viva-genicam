import { useMemo } from "react";
import type { UiGraph } from "../../xml_model/uigraph";
import { onListboxKeyDown } from "./listboxKeys";

interface CategoryListProps {
  graph: UiGraph | null;
  selectedCategory: string | null;
  onSelectCategory: (name: string) => void;
}

/** Narrow left column: flat list of top-level categories under root. */
export function CategoryList({
  graph,
  selectedCategory,
  onSelectCategory,
}: CategoryListProps) {
  const categories = useMemo(() => {
    if (!graph) return [];
    const root = graph.categories[graph.root_category];
    if (!root) return [];
    return root.features
      .map((name) => {
        const cat = graph.categories[name];
        if (!cat) return null;
        // Count direct non-category children
        const featureCount = cat.features.filter((f) => {
          const node = graph.nodes_by_name[f];
          return node && node.kind !== "Category";
        }).length;
        const subCatCount = cat.features.filter((f) => graph.categories[f]).length;
        return { name, displayName: cat.display_name, featureCount, subCatCount };
      })
      .filter(Boolean) as Array<{
        name: string;
        displayName: string;
        featureCount: number;
        subCatCount: number;
      }>;
  }, [graph]);

  if (!graph) {
    return (
      <div className="cat-list cat-list--empty">
        <span className="cat-list__hint">Load XML to browse</span>
      </div>
    );
  }

  return (
    <nav className="cat-list" role="listbox" aria-label="Categories" onKeyDown={onListboxKeyDown}>
      <div className="cat-list__header">Categories</div>
      {categories.map((cat) => {
        const isActive = cat.name === selectedCategory;
        return (
          <button
            key={cat.name}
            type="button"
            role="option"
            aria-selected={isActive}
            className={`cat-list__item${isActive ? " cat-list__item--active" : ""}`}
            onClick={() => onSelectCategory(cat.name)}
            title={cat.displayName}
          >
            <span className="cat-list__icon">{"\u25A6"}</span>
            <span className="cat-list__name">{cat.displayName}</span>
            <span className="cat-list__count">
              {cat.featureCount}
              {cat.subCatCount > 0 ? `+${cat.subCatCount}` : ""}
            </span>
          </button>
        );
      })}
    </nav>
  );
}
