import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ChangeEvent, KeyboardEvent } from "react";
import type { Diag, ParseXmlResponse, UiGraph, UiNode } from "../../xml_model/uigraph";
import type { NodeValue } from "../../xml_model/values";
import { useToast } from "../../context/ToastContext";
import { formatErrorMessage } from "../../device/errorMessage";
import { useFeature, useFeatureStore } from "../../device/FeatureStoreContext";
import { TauriProvider, type XmlModelProvider } from "../../xml_model/provider";
import { isUnknownKind, nodeDisplayName, nodeKindCssKey, nodeKindIcon } from "../../xml_model/helpers";
import { useDraftValues } from "../../state/useDraftValues";
import { Search, X } from "lucide-react";
import { useSplitter } from "../Layout/useSplitter";
import { CategoryList } from "./CategoryList";
import { FeatureList } from "./FeatureList";
import { categoryPath, formatLiveValue, graphSummary } from "./treeUtils";
import { useInView } from "./useInView";
import { countApplicableDrafts, formatBatchProgress } from "./batchApplyUtils";
import { buildLiveValuePreset, presetValueToDraft } from "./presetUtils";
import { FeaturePanel } from "./FeaturePanel";

// T7.1 — visibility levels; rank determines filtering inclusivity
export type VisibilityFilter = "Beginner" | "Expert" | "Guru" | "All";

/** The most search results listed; the header says when there are more. */
const SEARCH_LIMIT = 200;

const VISIBILITY_RANK: Record<string, number> = {
  Beginner: 0,
  Expert: 1,
  Guru: 2,
};

export function visibilityPassesFilter(
  nodeVisibility: string | undefined,
  filter: VisibilityFilter
): boolean {
  if (filter === "All") return true;
  if (!nodeVisibility) return true; // nodes without visibility always show
  const nodeRank = VISIBILITY_RANK[nodeVisibility] ?? 2;
  const filterRank = VISIBILITY_RANK[filter] ?? 0;
  return nodeRank <= filterRank;
}

type ParseStatus =
  | { kind: "idle" }
  | { kind: "loading"; fileName: string }
  | { kind: "error"; message: string }
  | { kind: "ready"; fileName: string };

interface FeatureBrowserPageProps {
  externalModel?: ParseXmlResponse | null;
  isConnected?: boolean;
  /**
   * Whether the Feature Browser is the visible tab. The page stays mounted
   * while hidden, so it must not act on keyboard shortcuts then.
   */
  isActive?: boolean;
}

export function FeatureBrowserPage({
  externalModel,
  isConnected = false,
  isActive = true,
}: FeatureBrowserPageProps = {}) {
  const toast = useToast();
  // Live state comes from the store; each control and row reads only what it
  // shows (backlog ST-29).
  const store = useFeatureStore();
  const rootRef = useRef<HTMLDivElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const presetInputRef = useRef<HTMLInputElement | null>(null);
  // T7.2 — search input ref for Ctrl+F focus
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  // T7.2 — debounce timer
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const provider = useMemo<XmlModelProvider>(
    () => new TauriProvider(),
    []
  );

  const [graph, setGraph] = useState<UiGraph | null>(null);
  const [xmlText, setXmlText] = useState<string>("");
  const [selectedNodeName, setSelectedNodeName] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  // T7.2 — raw input value (not debounced)
  const [searchInput, setSearchInput] = useState("");
  // T7.2 — debounced query used for actual filtering
  const [searchText, setSearchText] = useState("");
  // T7.2 — keyboard nav index in search results
  const [searchFocusIndex, setSearchFocusIndex] = useState(-1);
  // T7.1 — visibility filter
  const [visibilityFilter, setVisibilityFilter] = useState<VisibilityFilter>("Beginner");
  const [hideUnknown] = useState(false);
  const [status, setStatus] = useState<ParseStatus>({ kind: "idle" });
  const [diags, setDiags] = useState<Diag[]>([]);
  const [summaryOverride, setSummaryOverride] = useState<string | null>(null);

  const { drafts, errors, setDraft, resetDraft, clearAllDrafts } = useDraftValues();
  const [batchProgress, setBatchProgress] = useState<{ done: number; total: number } | null>(null);

  const { size: categoryWidth, handleProps: categorySplitterProps } = useSplitter({
    storageKey: "viva-studio:category-width",
    defaultSize: 160,
    minSize: 120,
    maxSize: 360,
  });
  const { size: featureListWidth, handleProps: featureListSplitterProps } = useSplitter({
    storageKey: "viva-studio:feature-list-width",
    defaultSize: 240,
    minSize: 160,
    maxSize: 400,
  });

  const applyResponse = useCallback(
    (response: ParseXmlResponse, fileName: string) => {
      clearAllDrafts();
      setGraph(response.graph);
      setXmlText(response.xml);
      setSelectedNodeName(null);
      setDiags(response.diags || []);
      setSummaryOverride(
        graphSummary(response.graph, response.summary.node_count, response.summary.category_count)
      );
      setStatus({ kind: "ready", fileName });
      // Auto-select first category
      const root = response.graph.categories[response.graph.root_category];
      if (root && root.features.length > 0) {
        const firstCat = root.features.find((f) => response.graph.categories[f]);
        setSelectedCategory(firstCat ?? null);
      }
    },
    [clearAllDrafts]
  );

  // Close overflow menu on outside click
  useEffect(() => {
    if (!menuOpen) return;
    function onClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [menuOpen]);

  // T7.2 — debounce search by 150 ms
  const handleSearchChange = useCallback((value: string) => {
    setSearchInput(value);
    setSearchFocusIndex(-1);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      setSearchText(value);
    }, 150);
  }, []);

  useEffect(() => {
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, []);

  useEffect(() => {
    let isMounted = true;

    if (provider.getCurrentModel) {
      provider
        .getCurrentModel()
        .then((response) => {
          if (isMounted && response) {
            applyResponse(response, response.summary.root_category || "Current Model");
          }
        })
        .catch(() => {});
    }

    return () => { isMounted = false; };
  }, [applyResponse, provider]);

  // A newly connected camera's model replaces the page's, while rendering
  // rather than in an effect that would render twice.
  const [appliedModel, setAppliedModel] = useState<ParseXmlResponse | null>(null);
  if (externalModel && externalModel !== appliedModel) {
    setAppliedModel(externalModel);
    applyResponse(externalModel, externalModel.summary.root_category || "Device");
  }

  // T7.6 — keyboard shortcuts, only while this tab is the one shown
  useEffect(() => {
    if (!isActive) return;
    function onKeyDown(e: globalThis.KeyboardEvent) {
      const ctrl = e.ctrlKey || e.metaKey;

      // Ctrl+F — focus search
      if (ctrl && e.key === "f") {
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }

      // Escape — clear search or deselect
      if (e.key === "Escape") {
        if (searchInput) {
          setSearchInput("");
          setSearchText("");
          setSearchFocusIndex(-1);
        } else {
          setSelectedNodeName(null);
        }
        return;
      }

      // Ctrl+Enter — apply if enabled
      if (ctrl && e.key === "Enter") {
        const applyBtn = rootRef.current?.querySelector<HTMLButtonElement>(
          ".editor-actions button:last-child:not(:disabled)"
        );
        applyBtn?.click();
        return;
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [isActive, searchInput]);

  const onLoadXml = useCallback(() => { fileInputRef.current?.click(); }, []);

  const onFileSelected = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      if (!file) return;
      setStatus({ kind: "loading", fileName: file.name });
      try {
        const xml = await file.text();
        const parsed = await provider.parseXml(xml);
        applyResponse(parsed, file.name);
      } catch (error) {
        setGraph(null);
        setXmlText("");
        setSelectedNodeName(null);
        setDiags([]);
        setSummaryOverride(null);
        setStatus({ kind: "error", message: formatErrorMessage(error) });
      } finally {
        event.target.value = "";
      }
    },
    [applyResponse, provider]
  );

  // T7.3 — Export preset: download current drafts as JSON
  const onExportPreset = useCallback(() => {
    if (!graph) return;
    const data = JSON.stringify(drafts, null, 2);
    const blob = new Blob([data], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "genicam-preset.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [graph, drafts]);

  // FB-05 — Export live state: read every feature and download it as JSON
  const onExportLiveState = useCallback(async () => {
    if (!graph || !isConnected) return;
    const names = Object.values(graph.nodes_by_name)
      .filter((node) => node.kind !== "Category")
      .map((node) => node.name);
    let states;
    try {
      states = await store.load(names);
    } catch (e) {
      toast.addToast("error", `Export failed: ${formatErrorMessage(e)}`);
      return;
    }
    const preset = buildLiveValuePreset(states);
    const data = JSON.stringify(preset, null, 2);
    const blob = new Blob([data], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "genicam-state.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [graph, isConnected, store, toast]);

  // T7.3 — Import preset: load JSON and restore drafts
  const onImportPreset = useCallback(() => {
    if (!graph) return;
    presetInputRef.current?.click();
  }, [graph]);

  const onPresetFileSelected = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      if (!file || !graph) return;
      try {
        const text = await file.text();
        const imported = JSON.parse(text) as Record<string, unknown>;
        Object.entries(imported).forEach(([name, raw]) => {
          const node = graph.nodes_by_name[name];
          if (!node) return;
          const value = presetValueToDraft(node, raw);
          if (value !== undefined) setDraft(node, value);
        });
      } catch {
        // Silently ignore malformed preset files
      } finally {
        event.target.value = "";
      }
    },
    [graph, setDraft]
  );

  const summary = useMemo(() => {
    if (summaryOverride) return summaryOverride;
    if (!graph) return "No model loaded";
    const nodeCount = Object.keys(graph.nodes_by_name ?? {}).length;
    const categoryCount = Object.keys(graph.categories ?? {}).length;
    return graphSummary(graph, nodeCount, categoryCount);
  }, [graph, summaryOverride]);

  const breadcrumb = useMemo(() => {
    const parts: Array<{ name: string; label: string }> = [];
    if (!graph) return parts;
    const path = categoryPath(graph, selectedCategory ?? graph.root_category);
    for (const name of path.length > 0 ? path : [graph.root_category]) {
      const cat = graph.categories[name];
      if (cat) parts.push({ name, label: cat.display_name });
    }
    if (selectedNodeName && selectedNodeName !== selectedCategory) {
      const node = graph.nodes_by_name[selectedNodeName];
      if (node) parts.push({ name: selectedNodeName, label: nodeDisplayName(node) });
    }
    return parts;
  }, [graph, selectedCategory, selectedNodeName]);

  const selectedNode = useMemo<UiNode | null>(() => {
    if (!graph || !selectedNodeName) return null;
    return graph.nodes_by_name[selectedNodeName] ?? null;
  }, [graph, selectedNodeName]);

  const selectedDraftValue = selectedNode ? drafts[selectedNode.name] : undefined;
  const selectedDraftErrors = selectedNode ? errors[selectedNode.name] ?? [] : [];
  const selectedHasDraft = selectedNode
    ? Object.prototype.hasOwnProperty.call(drafts, selectedNode.name)
    : false;

  // The editors show the camera's value until the user edits it; only an edit
  // makes a draft, so selecting a feature never queues a write.
  const selectedLiveState = useFeature(
    selectedNode && selectedNode.kind !== "Category" ? selectedNode.name : null,
  );

  const onSelectNode = useCallback((name: string) => {
    setSelectedNodeName(name);
    setSearchFocusIndex(-1);
  }, []);

  const onDraftChange = useCallback(
    (value: NodeValue) => { if (selectedNode) setDraft(selectedNode, value); },
    [selectedNode, setDraft]
  );

  const onDraftReset = useCallback(() => {
    if (selectedNode) resetDraft(selectedNode.name);
  }, [resetDraft, selectedNode]);

  const canApply = Boolean(provider.applyNodeValue);
  const canExecute = Boolean(provider.executeCommand);

  const applyDisabledReason = useMemo(() => {
    if (!provider.applyNodeValue) return "Offline mode: will be enabled when connected to a device.";
    if (!selectedHasDraft) return "No draft value to apply.";
    if (selectedDraftErrors.length > 0) return "Resolve validation errors before applying.";
    return "";
  }, [provider.applyNodeValue, selectedDraftErrors.length, selectedHasDraft]);

  const executeDisabledReason = useMemo(() => {
    if (!provider.executeCommand) return "Offline mode: will be enabled when connected to a device.";
    return "";
  }, [provider.executeCommand]);

  const onApply = useCallback(async () => {
    if (!provider.applyNodeValue || !selectedNode) return;
    if (!selectedHasDraft || selectedDraftErrors.length > 0) return;
    const value = drafts[selectedNode.name];
    if (value === undefined) return;
    try {
      const result = await provider.applyNodeValue(selectedNode.name, value);
      // Store the refreshed state so the form mirrors what the device
      // actually accepted (clamping/rounding applies). Then clear the draft —
      // the editor falls back to `liveState.value`, which is now correct.
      store.set(selectedNode.name, result);
      resetDraft(selectedNode.name);
    } catch (e) {
      toast.addToast("error", `Apply failed: ${formatErrorMessage(e)}`);
    }
  }, [
    drafts,
    provider,
    selectedDraftErrors.length,
    selectedHasDraft,
    selectedNode,
    resetDraft,
    store,
    toast,
  ]);

  const onExecute = useCallback(async () => {
    if (!provider.executeCommand || !selectedNode) return;
    try {
      const result = await provider.executeCommand(selectedNode.name);
      if (!result.ok) {
        toast.addToast(
          "error",
          `${selectedNode.name} failed${result.error ? `: ${result.error}` : ""}`,
        );
        return;
      }
      toast.addToast("success", `${selectedNode.name} executed`);
      // Fold in any side-effect states (e.g. AcquisitionStatus.active after
      // AcquisitionStart) so the UI reflects the post-execute world without
      // a manual refresh.
      store.setMany(result.affected_states ?? {});
    } catch (e) {
      toast.addToast("error", `${selectedNode.name} failed: ${formatErrorMessage(e)}`);
    }
  }, [provider, selectedNode, store, toast]);

  const applicableDraftCount = useMemo(
    () => countApplicableDrafts(drafts, errors),
    [drafts, errors],
  );

  const handleBatchApply = useCallback(async () => {
    if (!provider.applyNodeValue || !graph) return;

    // Build ordered list of valid drafts (skip any with validation errors).
    const applicable: Array<{ name: string; value: NodeValue }> = [];
    for (const [name, value] of Object.entries(drafts)) {
      if ((errors[name] ?? []).length === 0) {
        applicable.push({ name, value });
      }
    }
    if (applicable.length === 0) return;

    setBatchProgress({ done: 0, total: applicable.length });

    const failed: string[] = [];
    for (let i = 0; i < applicable.length; i++) {
      const item = applicable[i];
      if (!item) continue;
      try {
        const result = await provider.applyNodeValue(item.name, item.value);
        store.set(item.name, result);
        resetDraft(item.name);
      } catch {
        // Keep failed drafts in state so the user can inspect and retry.
        failed.push(item.name);
      }
      setBatchProgress({ done: i + 1, total: applicable.length });
    }

    setBatchProgress(null);
    if (failed.length > 0) {
      toast.addToast(
        "error",
        `${failed.length} of ${applicable.length} writes failed (${failed.join(", ")}); their drafts are kept`,
      );
    }
  }, [provider, graph, drafts, errors, resetDraft, store, toast]);

  // T7.1+T7.2 — filtered search results; the list shows the first
  // SEARCH_LIMIT and says so.
  const { searchResults, searchMatchCount } = useMemo(() => {
    if (!graph || !searchText.trim()) return { searchResults: [] as UiNode[], searchMatchCount: 0 };
    const query = searchText.trim().toLowerCase();
    const matches = Object.values(graph.nodes_by_name).filter((node) => {
      if (hideUnknown && isUnknownKind(node.kind)) return false;
      if (!visibilityPassesFilter(node.visibility, visibilityFilter)) return false;
      const display = nodeDisplayName(node).toLowerCase();
      return node.name.toLowerCase().includes(query) || display.includes(query);
    });
    return { searchResults: matches.slice(0, SEARCH_LIMIT), searchMatchCount: matches.length };
  }, [graph, hideUnknown, searchText, visibilityFilter]);

  // T7.2 — keyboard nav in search results
  const handleSearchKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (searchResults.length === 0) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSearchFocusIndex((prev) =>
          prev < searchResults.length - 1 ? prev + 1 : prev
        );
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSearchFocusIndex((prev) => (prev > 0 ? prev - 1 : 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const idx = searchFocusIndex >= 0 ? searchFocusIndex : 0;
        const node = searchResults[idx];
        if (node) {
          onSelectNode(node.name);
          // Clear search after selecting via Enter
          setSearchInput("");
          setSearchText("");
          setSearchFocusIndex(-1);
        }
      }
    },
    [searchResults, searchFocusIndex, onSelectNode]
  );

  return (
    <div className="feature-browser" ref={rootRef}>
      {/* Hidden file inputs */}
      <input ref={fileInputRef} type="file" accept=".xml,text/xml" onChange={onFileSelected} hidden />
      <input ref={presetInputRef} type="file" accept=".json,application/json" onChange={onPresetFileSelected} hidden />

      {/* Compact toolbar */}
      <div className="browser-toolbar">
        {/* Search */}
        <div className="browser-toolbar__search">
          <input
            ref={searchInputRef}
            className="browser-toolbar__search-input"
            type="search"
            placeholder={"Filter features\u2026  Ctrl+F"}
            value={searchInput}
            onChange={(e) => handleSearchChange(e.target.value)}
            onKeyDown={handleSearchKeyDown}
          />
          {searchInput ? (
            <button
              type="button"
              className="browser-toolbar__search-clear"
              aria-label="Clear search"
              title="Clear search"
              onClick={() => {
                handleSearchChange("");
                searchInputRef.current?.focus();
              }}
            >
              <X size={14} aria-hidden="true" />
            </button>
          ) : (
            <Search className="browser-toolbar__search-icon" size={14} aria-hidden="true" />
          )}
        </div>

        <div className="browser-toolbar__sep" />

        {/* Visibility filter */}
        <div className="vis-filter" role="group" aria-label="Visibility filter">
          {(["Beginner", "Expert", "Guru", "All"] as VisibilityFilter[]).map((level) => (
            <button
              key={level}
              type="button"
              className={`vis-filter__btn${visibilityFilter === level ? " vis-filter__btn--active" : ""}`}
              onClick={() => setVisibilityFilter(level)}
              title={`Show ${level === "All" ? "all" : level.toLowerCase()} features`}
            >
              {level}
            </button>
          ))}
        </div>

        <div className="browser-toolbar__sep" />

        {/* Batch apply (inline icon) */}
        {canApply && applicableDraftCount > 0 && (
          <button
            type="button"
            className="btn btn--sm"
            onClick={handleBatchApply}
            disabled={batchProgress !== null}
            title={`Apply all ${applicableDraftCount} pending draft${applicableDraftCount !== 1 ? "s" : ""}`}
          >
            Apply {applicableDraftCount}
          </button>
        )}

        {/* Refresh */}
        {isConnected && (
          <button
            type="button"
            className="browser-toolbar__icon-btn"
            onClick={() => store.refresh()}
            title="Read the features on screen again"
          >
            {"\u21BB"}
          </button>
        )}

        {/* Overflow menu */}
        <div className="browser-toolbar__overflow" ref={menuRef}>
          <button
            type="button"
            className="browser-toolbar__icon-btn"
            onClick={() => setMenuOpen((p) => !p)}
            title="More actions"
            aria-expanded={menuOpen}
          >
            {"\u2261"}
          </button>
          {menuOpen && (
            <div className="browser-toolbar__menu">
              <button type="button" onClick={() => { onLoadXml(); setMenuOpen(false); }}>
                {"Load XML\u2026"}
              </button>
              <button
                type="button"
                disabled={!graph}
                onClick={() => { onExportPreset(); setMenuOpen(false); }}
              >
                Export Preset
              </button>
              <button
                type="button"
                disabled={!graph}
                onClick={() => { onImportPreset(); setMenuOpen(false); }}
              >
                Import Preset
              </button>
              <button
                type="button"
                disabled={!graph || !isConnected}
                onClick={() => { void onExportLiveState(); setMenuOpen(false); }}
              >
                Export State
              </button>
            </div>
          )}
        </div>

        <span className="browser-toolbar__summary">{summary}</span>
      </div>

      {/* Status strip */}
      {batchProgress !== null && (
        <div className="browser-status browser-status--info">
          {formatBatchProgress(batchProgress.done, batchProgress.total)}
        </div>
      )}
      {status.kind === "loading" && (
        <div className="browser-status browser-status--info browser-status--shimmer">
          Loading {status.fileName}{"\u2026"}
        </div>
      )}
      {status.kind === "error" && (
        <div className="browser-status browser-status--error">{status.message}</div>
      )}

      {/* Three-column body */}
      <div
        className="feature-browser__body"
        style={{ gridTemplateColumns: searchText.trim()
          ? `1fr 8px 1fr`
          : `${categoryWidth}px 8px ${featureListWidth}px 8px 1fr`
        }}
      >
        {searchText.trim() ? (
          /* Search mode: results replace categories + feature list */
          <>
            <aside className="pane pane--left">
              <div className="pane__scroll">
                <SearchResults
                  results={searchResults}
                  matchCount={searchMatchCount}
                  query={searchText.trim()}
                  selectedNodeName={selectedNodeName}
                  focusIndex={searchFocusIndex}
                  onSelectNode={onSelectNode}
                />
              </div>
            </aside>
            <div className="splitter-handle" />
          </>
        ) : (
          /* Normal mode: three columns */
          <>
            <aside className="pane pane--categories">
              <CategoryList
                graph={graph}
                selectedCategory={selectedCategory}
                onSelectCategory={setSelectedCategory}
              />
            </aside>
            <div {...categorySplitterProps} aria-label="Resize categories" />
            <aside className="pane pane--features">
              <FeatureList
                graph={graph}
                categoryName={selectedCategory}
                visibilityFilter={visibilityFilter}
                selectedNodeName={selectedNodeName}
                onSelectNode={onSelectNode}
                onSelectCategory={setSelectedCategory}
              />
            </aside>
            <div {...featureListSplitterProps} aria-label="Resize feature list" />
          </>
        )}

        <section className="pane pane--editor">
          <FeaturePanel
            graph={graph}
            selectedNode={selectedNode}
            xmlText={xmlText}
            diags={diags}
            draftValue={selectedDraftValue}
            draftErrors={selectedDraftErrors}
            hasDraft={selectedHasDraft}
            onDraftChange={onDraftChange}
            onDraftReset={onDraftReset}
            canApply={canApply}
            applyDisabledReason={applyDisabledReason}
            onApply={onApply}
            canExecute={canExecute}
            executeDisabledReason={executeDisabledReason}
            onExecute={onExecute}
            liveState={selectedLiveState}
            onSelectNode={onSelectNode}
          />
        </section>
      </div>

      {/* Breadcrumb status bar */}
      <div className="browser-breadcrumb">
        {breadcrumb.map((part, i) => (
          <span key={part.name}>
            {i > 0 && <span className="browser-breadcrumb__sep">{"\u203A"}</span>}
            <button
              type="button"
              className="browser-breadcrumb__link"
              onClick={() => {
                if (graph?.categories[part.name]) {
                  setSelectedCategory(part.name);
                } else {
                  onSelectNode(part.name);
                }
              }}
            >
              {part.label}
            </button>
          </span>
        ))}
        {status.kind === "ready" && (
          <span className="browser-breadcrumb__file">{status.fileName}</span>
        )}
      </div>
    </div>
  );
}

// ── Search results with highlight ────────────────────────────────────────────

interface SearchResultsProps {
  results: UiNode[];
  /** How many features matched; `results` holds the first SEARCH_LIMIT. */
  matchCount: number;
  query: string;
  selectedNodeName: string | null;
  focusIndex: number;
  onSelectNode: (name: string) => void;
}

function SearchResults({
  results,
  matchCount,
  query,
  selectedNodeName,
  focusIndex,
  onSelectNode,
}: SearchResultsProps) {
  return (
    <div className="search-results">
      <div className="search-results__header">
        {results.length === 0
          ? "No matches"
          : matchCount > results.length
            ? `First ${results.length} of ${matchCount} matches — refine the search`
            : `${results.length} result${results.length !== 1 ? "s" : ""}`}
      </div>
      {results.length === 0 ? (
        <div className="search-results__empty">No features match "{query}"</div>
      ) : (
        <ul>
          {results.map((node, idx) => (
            <SearchResultItem
              key={node.name}
              node={node}
              query={query}
              isActive={node.name === selectedNodeName}
              isKeyboardFocused={idx === focusIndex}
              onSelect={() => onSelectNode(node.name)}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

/** One search result, following its feature's live value while on screen. */
function SearchResultItem({
  node,
  query,
  isActive,
  isKeyboardFocused,
  onSelect,
}: {
  node: UiNode;
  query: string;
  isActive: boolean;
  isKeyboardFocused: boolean;
  onSelect: () => void;
}) {
  const [ref, inView] = useInView<HTMLButtonElement>();
  const state = useFeature(node.kind === "Category" || !inView ? null : node.name);
  const liveText = state ? formatLiveValue(state) : null;
  const classes = [
    "tree-item",
    isActive ? "tree-item--active" : "",
    isKeyboardFocused ? "tree-item--keyboard-focus" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <li>
      <button ref={ref} type="button" className={classes} onClick={onSelect}>
        <span className={`tree-item__icon tree-item__icon--${nodeKindCssKey(node.kind)}`}>
          {nodeKindIcon(node.kind)}
        </span>
        <span className="tree-item__label">{highlightMatch(nodeDisplayName(node), query)}</span>
        {liveText !== null ? (
          <span className="tree-item__live">{liveText}</span>
        ) : (
          <span className="tree-item__meta">{highlightMatch(node.name, query)}</span>
        )}
      </button>
    </li>
  );
}

// T7.2 — highlight the matched substring in a label
function highlightMatch(text: string, query: string): React.ReactNode {
  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  const idx = lowerText.indexOf(lowerQuery);
  if (idx === -1) return text;
  return (
    <>
      {text.slice(0, idx)}
      <mark className="search-match">{text.slice(idx, idx + query.length)}</mark>
      {text.slice(idx + query.length)}
    </>
  );
}
