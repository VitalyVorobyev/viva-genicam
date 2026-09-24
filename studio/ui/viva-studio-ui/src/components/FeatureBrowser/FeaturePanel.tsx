import { useMemo, useState } from "react";
import { useSplitter } from "../Layout/useSplitter";
import type { Diag, UiGraph, UiNode, UiNodeKind } from "../../xml_model/uigraph";
import type { NodeValue, ValueError } from "../../xml_model/values";
import type { FeatureState, NodeValueEntry } from "../../device/types";
import { isUnknownKind, nodeDisplayName, nodeKindLabel } from "../../xml_model/helpers";
import { BoolEditor } from "./editors/BoolEditor";
import { CommandView } from "./editors/CommandView";
import { EnumEditor } from "./editors/EnumEditor";
import { FloatEditor } from "./editors/FloatEditor";
import { IntegerEditor } from "./editors/IntegerEditor";
import { StringEditor } from "./editors/StringEditor";
import { UnknownDebugView } from "./editors/UnknownDebugView";

interface FeaturePanelProps {
  graph: UiGraph | null;
  selectedNode: UiNode | null;
  xmlText: string;
  diags: Diag[];
  draftValue: NodeValue | undefined;
  draftErrors: ValueError[];
  hasDraft: boolean;
  onDraftChange: (value: NodeValue) => void;
  onDraftReset: () => void;
  canApply: boolean;
  applyDisabledReason: string;
  onApply: () => void;
  canExecute: boolean;
  executeDisabledReason: string;
  onExecute: () => void;
  liveValue?: NodeValueEntry;
  /**
   * Authoritative live state of the selected node. When present, the editors
   * use it to drive enum options, numeric ranges, access-mode gating, and to
   * seed the form value when the draft is unset.
   */
  liveState?: FeatureState;
  onSelectNode?: (name: string) => void;
}

export function FeaturePanel({
  graph,
  selectedNode,
  xmlText,
  diags,
  draftValue,
  draftErrors,
  hasDraft,
  onDraftChange,
  onDraftReset,
  canApply,
  applyDisabledReason,
  onApply,
  canExecute,
  executeDisabledReason,
  onExecute,
  liveValue,
  liveState,
  onSelectNode,
}: FeaturePanelProps) {
  const [infoOpen, setInfoOpen] = useState(true);
  const [debugOpen, setDebugOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<"raw" | "debug" | "diagnostics">("debug");
  const { size: debugHeight, handleProps: debugSplitterProps } = useSplitter({
    storageKey: "viva-studio:debug-panel-height",
    defaultSize: 200,
    minSize: 80,
    maxSize: 500,
    orientation: "vertical",
  });

  const infoText = useMemo(() => {
    if (!selectedNode) return null;
    const info = [selectedNode.tooltip, selectedNode.comment, selectedNode.description]
      .filter(Boolean)
      .join("\n\n");
    return info || null;
  }, [selectedNode]);

  const diagnosticsPanel = renderDiagnostics(diags);
  const editable = selectedNode ? isEditableKind(selectedNode.kind) : false;
  const draftSummary =
    selectedNode && editable
      ? formatDraftSummary(selectedNode, draftValue, hasDraft)
      : null;

  if (!graph || !selectedNode) {
    return (
      <div className="feature-panel feature-panel--empty">
        <p>Select a feature to view details.</p>
      </div>
    );
  }

  const applyDisabled = !canApply || !hasDraft || draftErrors.length > 0;
  const applyTitle = applyDisabled
    ? applyDisabledReason || "Draft not ready."
    : "Apply draft to device  (Ctrl+Enter)";

  const kindLabel = nodeKindLabel(selectedNode.kind);
  const kindClass = kindBadgeClass(selectedNode.kind);

  return (
    <div
      className="feature-panel"
      style={{
        gridTemplateRows: debugOpen ? `1fr 8px ${debugHeight}px` : "1fr",
      }}
    >
      {/* Upper pane: editor content (scrollable) */}
      <div className="feature-panel__upper">
        <header className="feature-panel__header">
          <h2 className="feature-panel__name">{nodeDisplayName(selectedNode)}</h2>
          <div className="feature-panel__meta">
            <span className={`kind-badge ${kindClass}`}>{kindLabel}</span>
            <span className="feature-panel__raw-name">{selectedNode.name}</span>
            {/* A command or write-only node has no value: show its access
                mode alone, not the text "null". */}
            {liveValue !== undefined && liveValue.value !== null && (
              <span className="live-badge">
                <span className="live-badge__dot" />
                <span className="live-badge__value">{String(liveValue.value)}</span>
              </span>
            )}
            {(liveValue?.access_mode ?? selectedNode.access_mode) && (
              <span className="feature-panel__access">
                {liveValue?.access_mode ?? selectedNode.access_mode}
              </span>
            )}
            {selectedNode.visibility && (
              <span className="feature-panel__visibility">{selectedNode.visibility}</span>
            )}
          </div>
          {draftSummary && (
            <div className="draft-summary">
              <span className="draft-summary__dot" />
              {draftSummary}
            </div>
          )}
        </header>

        {infoText && (
          <section className="info" style={{ marginTop: "12px" }}>
            <button
              type="button"
              className="info__toggle"
              onClick={() => setInfoOpen((prev) => !prev)}
            >
              <span className="info__toggle-caret">{infoOpen ? "\u25BE" : "\u25B8"}</span>
              Info
            </button>
            {infoOpen && <pre className="info__content">{infoText}</pre>}
          </section>
        )}

        <section className="feature-panel__body">
          {renderEditor(
            selectedNode,
            draftValue,
            draftErrors,
            onDraftChange,
            canExecute,
            executeDisabledReason,
            onExecute,
            liveState
          )}
        </section>

        {editable && (
          <section className="editor-actions">
            <button
              type="button"
              className="btn--secondary"
              onClick={onDraftReset}
              disabled={!hasDraft}
            >
              Reset
            </button>
            <button
              type="button"
              className="btn"
              onClick={onApply}
              disabled={applyDisabled}
              title={applyTitle}
            >
              Apply
            </button>
          </section>
        )}

        {((selectedNode.dependencies?.length ?? 0) > 0 || (selectedNode.dependents?.length ?? 0) > 0) && (
          <section className="feature-panel__deps">
            {(selectedNode.dependencies?.length ?? 0) > 0 && (
              <div className="dep-group">
                <span className="dep-group__label">Depends on:</span>
                <div className="dep-group__links">
                  {selectedNode.dependencies!.map((dep) => (
                    <button
                      key={dep}
                      type="button"
                      className="dep-link"
                      onClick={() => onSelectNode?.(dep)}
                      title={`Navigate to ${dep}`}
                    >
                      {dep}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {(selectedNode.dependents?.length ?? 0) > 0 && (
              <div className="dep-group">
                <span className="dep-group__label">Invalidates:</span>
                <div className="dep-group__links">
                  {selectedNode.dependents!.map((dep) => (
                    <button
                      key={dep}
                      type="button"
                      className="dep-link dep-link--dependent"
                      onClick={() => onSelectNode?.(dep)}
                      title={`Navigate to ${dep}`}
                    >
                      {dep}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {selectedNode.expression && (
              <div className="dep-group">
                <span className="dep-group__label">Expression:</span>
                <code className="dep-expression">{selectedNode.expression}</code>
              </div>
            )}
          </section>
        )}

        {/* Developer toggle at the bottom of the editor pane */}
        <div className="feature-panel__debug-toggle-bar">
          <button
            type="button"
            className="feature-panel__debug-toggle"
            onClick={() => setDebugOpen((p) => !p)}
          >
            <span className="info__toggle-caret">{debugOpen ? "\u25BE" : "\u25B8"}</span>
            Developer
          </button>
        </div>
      </div>

      {/* Splitter + Debug pane (only when open) */}
      {debugOpen && (
        <>
          <div {...debugSplitterProps} />
          <div className="feature-panel__debug-content">
            <div className="tabs tabs--sm">
              {(["raw", "debug", "diagnostics"] as const).map((t) => (
                <button
                  key={t}
                  type="button"
                  className={activeTab === t ? "tab tab--active" : "tab"}
                  onClick={() => setActiveTab(t)}
                >
                  {tabLabel(t)}
                </button>
              ))}
            </div>
            <div className="tab-panel tab-panel--dev">
              {activeTab === "raw" ? (
                <pre className="dev-pre">{extractNodeXml(xmlText, selectedNode.name) || "(no XML match)"}</pre>
              ) : activeTab === "debug" ? (
                <pre className="dev-pre">{JSON.stringify(selectedNode, null, 2)}</pre>
              ) : (
                diagnosticsPanel
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
}

function tabLabel(t: "raw" | "debug" | "diagnostics"): string {
  switch (t) {
    case "raw":         return "Raw XML";
    case "debug":       return "Model Debug";
    case "diagnostics": return "Diagnostics";
  }
}

function kindBadgeClass(kind: UiNodeKind): string {
  if (typeof kind !== "string") return "kind-badge--unknown";
  switch (kind) {
    case "Integer":     return "kind-badge--integer";
    case "Float":       return "kind-badge--float";
    case "Boolean":     return "kind-badge--boolean";
    case "String":      return "kind-badge--string";
    case "Enumeration": return "kind-badge--enum";
    case "Command":     return "kind-badge--command";
    case "Register":    return "kind-badge--register";
    case "Category":    return "kind-badge--category";
    default:            return "kind-badge--unknown";
  }
}

function renderEditor(
  node: UiNode,
  draftValue: NodeValue | undefined,
  draftErrors: ValueError[],
  onDraftChange: (value: NodeValue) => void,
  canExecute: boolean,
  executeDisabledReason: string,
  onExecute: () => void,
  liveState: FeatureState | undefined
) {
  if (isUnknownKind(node.kind)) {
    return <UnknownDebugView raw={node.raw} />;
  }

  switch (node.kind) {
    case "Integer":
      return (
        <IntegerEditor
          node={node}
          value={draftValue}
          errors={draftErrors}
          onChange={onDraftChange}
          liveState={liveState}
        />
      );
    case "Float":
      return (
        <FloatEditor
          node={node}
          value={draftValue}
          errors={draftErrors}
          onChange={onDraftChange}
          liveState={liveState}
        />
      );
    case "Enumeration":
      return (
        <EnumEditor
          node={node}
          value={draftValue}
          errors={draftErrors}
          onChange={onDraftChange}
          liveState={liveState}
        />
      );
    case "Boolean":
      return (
        <BoolEditor value={draftValue} errors={draftErrors} onChange={onDraftChange} />
      );
    case "String":
      return (
        <StringEditor value={draftValue} errors={draftErrors} onChange={onDraftChange} />
      );
    case "Command":
      return (
        <CommandView
          canExecute={canExecute}
          onExecute={onExecute}
          disabledReason={executeDisabledReason}
          liveState={liveState}
        />
      );
    case "Register":
      return (
        <div className="editor__hint">Register node — read-only in offline mode.</div>
      );
    case "Category":
      return (
        <div className="editor__hint">Category — select a child feature to edit.</div>
      );
    default:
      return null;
  }
}

function isEditableKind(kind: UiNodeKind): boolean {
  return (
    kind === "Integer" ||
    kind === "Float" ||
    kind === "Enumeration" ||
    kind === "Boolean" ||
    kind === "String"
  );
}

function formatDraftSummary(
  node: UiNode,
  value: NodeValue | undefined,
  hasDraft: boolean
): string {
  if (!hasDraft || value === null || value === undefined) return "Draft: unset (offline)";
  if (node.kind === "Boolean" && typeof value === "boolean")
    return `Draft: ${value ? "enabled" : "disabled"}`;
  if (node.kind === "String" && typeof value === "string")
    return value.length === 0 ? 'Draft: "" (empty)' : `Draft: ${value}`;
  if (node.kind === "Enumeration" && isEnumValue(value)) {
    const match = node.enum_entries?.find((e) => e.name === value.enumName);
    const label = match?.display_name ?? match?.name ?? value.enumName;
    return `Draft: ${label}`;
  }
  if (typeof value === "number") return `Draft: ${value}`;
  return "Draft: (unrecognized)";
}

function renderDiagnostics(diags: Diag[]) {
  if (!diags || diags.length === 0) {
    return <div className="diagnostics diagnostics--empty">No parser diagnostics.</div>;
  }
  return (
    <ul className="diagnostics">
      {diags.map((diag, index) => (
        <li key={`${diag.level}-${diag.message}-${index}`}>
          <span className={`diagnostics__level diagnostics__level--${diag.level}`}>
            {diag.level.toUpperCase()}
          </span>
          <span className="diagnostics__message">{diag.message}</span>
          {diag.node && <span className="diagnostics__node">({diag.node})</span>}
        </li>
      ))}
    </ul>
  );
}

/** Extract the XML block for a given node name from the full XML text. */
function extractNodeXml(xmlText: string, nodeName: string): string | null {
  if (!xmlText || !nodeName) return null;
  // Try to find an element with Name="nodeName" attribute
  const nameAttr = `Name="${nodeName}"`;
  const idx = xmlText.indexOf(nameAttr);
  if (idx === -1) return null;
  // Walk backward to find the opening <
  let start = idx;
  while (start > 0 && xmlText[start] !== "<") start--;
  // Find the tag name
  const tagMatch = xmlText.slice(start).match(/^<(\w+)/);
  if (!tagMatch) return null;
  const tag = tagMatch[1];
  // Find the closing tag
  const closeTag = `</${tag}>`;
  const closeIdx = xmlText.indexOf(closeTag, start);
  if (closeIdx === -1) {
    // Self-closing? Find />
    const selfClose = xmlText.indexOf("/>", idx);
    if (selfClose !== -1) return xmlText.slice(start, selfClose + 2).trim();
    return null;
  }
  return xmlText.slice(start, closeIdx + closeTag.length).trim();
}

function isEnumValue(value: NodeValue): value is { enumName: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "enumName" in value &&
    typeof (value as { enumName?: unknown }).enumName === "string"
  );
}
