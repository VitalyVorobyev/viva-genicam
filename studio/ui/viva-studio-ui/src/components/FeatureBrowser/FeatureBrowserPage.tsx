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
import type { FeatureState, NodeValueEntry } from "../../device/types";
import { useToast } from "../../context/ToastContext";
import { TauriProvider, type XmlModelProvider } from "../../xml_model/provider";
import { isUnknownKind, nodeDisplayName, nodeKindCssKey, nodeKindIcon } from "../../xml_model/helpers";
import { useDraftValues } from "../../state/useDraftValues";
import { useSplitter } from "../Layout/useSplitter";
import { CategoryList } from "./CategoryList";
import { FeatureList } from "./FeatureList";
import { formatLiveValue } from "./treeUtils";
import { countApplicableDrafts, formatBatchProgress } from "./batchApplyUtils";
import { buildLiveValuePreset } from "./presetUtils";
import { FeaturePanel } from "./FeaturePanel";

// T7.1 — visibility levels; rank determines filtering inclusivity
export type VisibilityFilter = "Beginner" | "Expert" | "Guru" | "All";

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
  liveValues?: Map<string, NodeValueEntry>;
  /**
   * Authoritative live state map keyed by node name. Preferred over
   * `liveValues` for everything except legacy call-sites that only need the
   * value/access_mode pair. When a node is in this map, the Feature Browser
   * uses its `numeric` range, `enum_available`, and `access_mode` to drive the
   * editor controls — no fallback to static XML.
   */
  liveStates?: Map<string, FeatureState>;
  /** Merge a single `FeatureState` back into the live cache (post-apply/execute). */
  onMergeState?: (name: string, state: FeatureState) => void;
  isConnected?: boolean;
  onRefreshAll?: () => Promise<void>;
}

export function FeatureBrowserPage({
  externalModel,
  liveValues = new Map(),
  liveStates = new Map(),
  onMergeState,
  isConnected = false,
  onRefreshAll,
}: FeatureBrowserPageProps = {}) {
  const toast = useToast();
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
        `${response.summary.node_count} nodes · ${response.summary.category_count} cat`
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

  useEffect(() => {
    if (externalModel) {
      applyResponse(externalModel, externalModel.summary.root_category || "Device");
    }
  }, [externalModel, applyResponse]);

  // T7.6 — keyboard shortcuts
  useEffect(() => {
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
        const applyBtn = document.querySelector<HTMLButtonElement>(
          ".editor-actions button:last-child:not(:disabled)"
        );
        applyBtn?.click();
        return;
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [searchInput]);

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

  // FB-05 — Export live state: download all current live values as JSON
  const onExportLiveState = useCallback(() => {
    if (!graph || !liveValues || liveValues.size === 0) return;
    const preset = buildLiveValuePreset(liveValues);
    const data = JSON.stringify(preset, null, 2);
    const blob = new Blob([data], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "genicam-state.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [graph, liveValues]);

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
        const imported = JSON.parse(text) as Record<string, NodeValue>;
        Object.entries(imported).forEach(([name, value]) => {
          const node = graph.nodes_by_name[name];
          if (node && value !== undefined) {
            setDraft(node, value);
          }
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
    return `${nodeCount} nodes · ${categoryCount} cat`;
  }, [graph, summaryOverride]);

  const breadcrumb = useMemo(() => {
    const parts: Array<{ name: string; label: string }> = [];
    if (!graph) return parts;
    const rootCat = graph.categories[graph.root_category];
    if (rootCat) parts.push({ name: graph.root_category, label: rootCat.display_name });
    if (selectedCategory && selectedCategory !== graph.root_category) {
      const cat = graph.categories[selectedCategory];
      if (cat) parts.push({ name: selectedCategory, label: cat.display_name });
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

  const selectedLiveValue = selectedNode ? liveValues.get(selectedNode.name) : undefined;
  const selectedLiveState = selectedNode ? liveStates.get(selectedNode.name) : undefined;

  const onSelectNode = useCallback(
    (name: string) => {
      setSelectedNodeName(name);
      setSearchFocusIndex(-1);
      if (!isConnected) return;
      const node = graph?.nodes_by_name[name];
      if (!node) return;
      if (Object.prototype.hasOwnProperty.call(drafts, name)) return;
      const live = liveValues.get(name);
      if (!live) return;
      const nodeValue = liveValueToNodeValue(live.value);
      if (nodeValue !== null) setDraft(node, nodeValue);
    },
    [graph, drafts, isConnected, liveValues, setDraft]
  );

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
      // Merge the refreshed state into the cache so the form mirrors what
      // the device actually accepted (clamping/rounding applies). Then clear
      // the draft — the editor falls back to `liveState.value`, which is now
      // correct, rather than rendering "(unset)".
      if (onMergeState) {
        onMergeState(selectedNode.name, result);
      }
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
    onMergeState,
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
      if (onMergeState && result.affected_states) {
        for (const [name, state] of Object.entries(result.affected_states)) {
          onMergeState(name, state);
        }
      }
    } catch (e) {
      toast.addToast("error", `${selectedNode.name} failed: ${formatErrorMessage(e)}`);
    }
  }, [provider, selectedNode, onMergeState, toast]);

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

    for (let i = 0; i < applicable.length; i++) {
      const item = applicable[i];
      if (!item) continue;
      try {
        const result = await provider.applyNodeValue(item.name, item.value);
        if (onMergeState) onMergeState(item.name, result);
        resetDraft(item.name);
      } catch {
        // Keep failed drafts in state so the user can inspect and retry.
      }
      setBatchProgress({ done: i + 1, total: applicable.length });
    }

    setBatchProgress(null);
  }, [provider, graph, drafts, errors, resetDraft, onMergeState]);

  // T7.1+T7.2 — filtered search results
  const searchResults = useMemo(() => {
    if (!graph || !searchText.trim()) return [] as UiNode[];
    const query = searchText.trim().toLowerCase();
    return Object.values(graph.nodes_by_name)
      .filter((node) => {
        if (hideUnknown && isUnknownKind(node.kind)) return false;
        if (!visibilityPassesFilter(node.visibility, visibilityFilter)) return false;
        const display = nodeDisplayName(node).toLowerCase();
        return node.name.toLowerCase().includes(query) || display.includes(query);
      })
      .slice(0, 200);
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
    <div className="feature-browser">
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
              onClick={() => {
                handleSearchChange("");
                searchInputRef.current?.focus();
              }}
            >
              {"\u00D7"}
            </button>
          ) : (
            <svg className="browser-toolbar__search-icon" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <circle cx="7" cy="7" r="4.5" stroke="currentColor" strokeWidth="1.5" />
              <line x1="10.5" y1="10.5" x2="14" y2="14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
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
        {onRefreshAll && isConnected && (
          <button
            type="button"
            className="browser-toolbar__icon-btn"
            onClick={onRefreshAll}
            title="Refresh all node values"
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
                disabled={!graph || !liveValues || liveValues.size === 0}
                onClick={() => { onExportLiveState(); setMenuOpen(false); }}
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
          : `160px ${featureListWidth}px 8px 1fr`
        }}
      >
        {searchText.trim() ? (
          /* Search mode: results replace categories + feature list */
          <>
            <aside className="pane pane--left">
              <div className="pane__scroll">
                <SearchResults
                  results={searchResults}
                  query={searchText.trim()}
                  selectedNodeName={selectedNodeName}
                  focusIndex={searchFocusIndex}
                  onSelectNode={onSelectNode}
                  liveValues={liveValues}
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
            <aside className="pane pane--features">
              <FeatureList
                graph={graph}
                categoryName={selectedCategory}
                visibilityFilter={visibilityFilter}
                selectedNodeName={selectedNodeName}
                onSelectNode={onSelectNode}
                onSelectCategory={setSelectedCategory}
                liveValues={liveValues}
              />
            </aside>
            <div {...featureListSplitterProps} />
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
            liveValue={selectedLiveValue}
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
  query: string;
  selectedNodeName: string | null;
  focusIndex: number;
  onSelectNode: (name: string) => void;
  liveValues?: Map<string, NodeValueEntry>;
}

function SearchResults({
  results,
  query,
  selectedNodeName,
  focusIndex,
  onSelectNode,
  liveValues,
}: SearchResultsProps) {
  return (
    <div className="search-results">
      <div className="search-results__header">
        {results.length === 0
          ? "No matches"
          : `${results.length} result${results.length !== 1 ? "s" : ""}`}
      </div>
      {results.length === 0 ? (
        <div className="search-results__empty">No features match "{query}"</div>
      ) : (
        <ul>
          {results.map((node, idx) => {
            const displayName = nodeDisplayName(node);
            const isActive = node.name === selectedNodeName;
            const isKeyboardFocused = idx === focusIndex;
            const classes = [
              "tree-item",
              isActive ? "tree-item--active" : "",
              isKeyboardFocused ? "tree-item--keyboard-focus" : "",
            ]
              .filter(Boolean)
              .join(" ");

            const kindKey = nodeKindCssKey(node.kind);
            const icon = nodeKindIcon(node.kind);
            const liveEntry = liveValues?.get(node.name);
            const liveText = liveEntry !== undefined ? formatLiveValue(liveEntry) : null;

            return (
              <li key={node.name}>
                <button
                  type="button"
                  className={classes}
                  onClick={() => onSelectNode(node.name)}
                >
                  <span className={`tree-item__icon tree-item__icon--${kindKey}`}>
                    {icon}
                  </span>
                  <span className="tree-item__label">
                    {highlightMatch(displayName, query)}
                  </span>
                  {liveText !== null ? (
                    <span className="tree-item__live">{liveText}</span>
                  ) : (
                    <span className="tree-item__meta">
                      {highlightMatch(node.name, query)}
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
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

// ── Helpers ──────────────────────────────────────────────────────────────────

/** `null` (a write-only or command node) stays `null`: there is no draft to seed. */
function liveValueToNodeValue(
  raw: number | string | boolean | null
): import("../../xml_model/values").NodeValue {
  return raw as import("../../xml_model/values").NodeValue;
}

function formatErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const message = (error as { message?: string }).message;
    const details = (error as { details?: string }).details;
    if (message && details) return `${message} (${details})`;
    if (message) return message;
    if (details) return details;
  }
  return String(error);
}
