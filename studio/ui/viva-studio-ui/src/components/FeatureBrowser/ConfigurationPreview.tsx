import { useEffect, useId, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { StreamerInfo } from "../../device/types";
import { ViewerCanvas } from "../ImageViewer/ViewerCanvas";

const STORAGE_KEY = "configuration-preview-expanded";
const ignoreFrameStats = () => {};

export interface ConfigurationPreviewProps {
  streamerInfo: StreamerInfo | null;
  deviceId: string | null;
  isConnected: boolean;
  isAcquiring: boolean;
  isActive: boolean;
  /** Allows the surrounding toolbar to keep configuration visible on Start. */
  onExpandedChange?: (expanded: boolean) => void;
}

/** A fit-only live reference while editing features; acquisition stays explicit. */
export function ConfigurationPreview({
  streamerInfo,
  deviceId,
  isConnected,
  isAcquiring,
  isActive,
  onExpandedChange,
}: ConfigurationPreviewProps) {
  const panelId = useId();
  const [expanded, setExpanded] = useState(() => {
    try { return localStorage.getItem(STORAGE_KEY) === "true"; }
    catch { return false; }
  });
  useEffect(() => { onExpandedChange?.(expanded); }, [expanded, onExpandedChange]);
  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    try { localStorage.setItem(STORAGE_KEY, String(next)); }
    catch { /* A storage restriction must not prevent using the preview. */ }
  };

  return (
    <section className="configuration-preview" aria-label="Configuration image preview">
      <button
        type="button"
        className="configuration-preview__toggle"
        aria-expanded={expanded}
        aria-controls={panelId}
        onClick={toggle}
      >
        {expanded ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}
        Image preview
        <span className="configuration-preview__hint">Fit to view</span>
      </button>
      {expanded && (
        <div id={panelId} className="configuration-preview__canvas">
          {isActive && isConnected && deviceId && streamerInfo ? (
            <ViewerCanvas
              key={`${deviceId}:${streamerInfo.ws_url}`}
              wsUrl={streamerInfo.ws_url}
              streamKey={deviceId}
              onFrameStats={ignoreFrameStats}
              isStreaming={isAcquiring}
            />
          ) : (
            <p className="configuration-preview__empty">
              {!isConnected ? "Connect a camera to preview its image." : "Start acquisition from the toolbar to preview images."}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
