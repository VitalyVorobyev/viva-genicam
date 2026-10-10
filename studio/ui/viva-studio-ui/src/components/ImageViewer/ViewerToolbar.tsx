import type { ImageRect } from "./roiUtils";
import { formatRoiLabel, roiIsValid } from "./roiUtils";
import {
  Ruler,
  BarChart3,
  Scan,
  Maximize,
  Camera,
} from "lucide-react";

const ICON_SIZE = 15;

interface ViewerToolbarProps {
  zoomLabel?: string;
  deviceName?: string;
  onResetZoom: () => void;
  onZoomTo100?: () => void;
  onSnapshot?: () => void;
  showHistogram?: boolean;
  onToggleHistogram?: () => void;
  showRoiTool?: boolean;
  onToggleRoiTool?: () => void;
  roiRect?: ImageRect | null;
  onApplyRoi?: () => void;
  showLineTool?: boolean;
  onToggleLineTool?: () => void;
}

export function ViewerToolbar({
  zoomLabel,
  deviceName,
  onResetZoom,
  onZoomTo100,
  onSnapshot,
  showHistogram,
  onToggleHistogram,
  showRoiTool,
  onToggleRoiTool,
  roiRect,
  onApplyRoi,
  showLineTool,
  onToggleLineTool,
}: ViewerToolbarProps) {
  const titleClass = deviceName
    ? "iv-toolbar__title iv-toolbar__title--connected"
    : "iv-toolbar__title";

  const hasValidRoi = roiRect !== null && roiRect !== undefined && roiIsValid(roiRect);

  return (
    <div className="iv-toolbar">
      <span className={titleClass}>{deviceName ?? "Image Viewer"}</span>
      <span className="iv-toolbar__spacer" />

      {/* ROI apply button — only visible when a valid selection exists */}
      {hasValidRoi && onApplyRoi !== undefined && (
        <button
          type="button"
          className="iv-toolbar__roi-apply"
          onClick={onApplyRoi}
          title={`Apply ROI: ${formatRoiLabel(roiRect!)}`}
          aria-label="Apply selected ROI"
        >
          Apply {roiRect!.w}×{roiRect!.h}
        </button>
      )}

      {/* ── Analysis tools ── */}
      {onToggleLineTool !== undefined && (
        <button
          type="button"
          className={`iv-toolbar__btn${showLineTool ? " iv-toolbar__btn--active" : ""}`}
          onClick={onToggleLineTool}
          title="Line profile tool"
          aria-label="Toggle line profile tool"
          aria-pressed={showLineTool}
        >
          <Ruler size={ICON_SIZE} />
        </button>
      )}
      {onToggleRoiTool !== undefined && (
        <button
          type="button"
          className={`iv-toolbar__btn${showRoiTool ? " iv-toolbar__btn--active" : ""}`}
          onClick={onToggleRoiTool}
          title="ROI selection tool"
          aria-label="Toggle ROI selection tool"
          aria-pressed={showRoiTool}
        >
          <Scan size={ICON_SIZE} />
        </button>
      )}
      {onToggleHistogram !== undefined && (
        <button
          type="button"
          className={`iv-toolbar__btn${showHistogram ? " iv-toolbar__btn--active" : ""}`}
          onClick={onToggleHistogram}
          title="Toggle histogram"
          aria-label="Toggle histogram"
          aria-pressed={showHistogram}
        >
          <BarChart3 size={ICON_SIZE} />
        </button>
      )}

      <div className="iv-toolbar__sep" />

      {/* ── Zoom controls ── */}
      <button
        type="button"
        className="iv-toolbar__btn"
        onClick={onResetZoom}
        title="Fit to window"
        aria-label="Fit to window"
      >
        <Maximize size={ICON_SIZE} />
      </button>
      {onZoomTo100 !== undefined && (
        <button
          type="button"
          className={`iv-toolbar__btn${zoomLabel === "100%" ? " iv-toolbar__btn--active" : ""}`}
          onClick={onZoomTo100}
          title="Zoom to 100% (1:1 pixels)"
          aria-label="Zoom to 100%"
        >
          <span className="iv-toolbar__btn-label">1:1</span>
        </button>
      )}
      {zoomLabel !== undefined && (
        <button
          type="button"
          className="iv-toolbar__zoom"
          onClick={onResetZoom}
          title="Reset zoom"
          aria-label="Reset zoom"
        >
          {zoomLabel}
        </button>
      )}

      <div className="iv-toolbar__sep" />

      {/* ── Capture tools ── */}
      <button
        type="button"
        className="iv-toolbar__btn"
        disabled={!onSnapshot}
        onClick={onSnapshot}
        title="Save snapshot"
        aria-label="Save snapshot"
      >
        <Camera size={ICON_SIZE} />
      </button>
    </div>
  );
}
