import type React from "react";
import { HistogramOverlay } from "./HistogramOverlay";
import { LineProfilePanel } from "./LineProfilePanel";
import type { LineSegment } from "./lineProfileUtils";
import type { NativeFrame } from "./nativeFrame";

interface AnalysisPanelProps {
  showHistogram: boolean;
  showLineTool: boolean;
  lineSegment: LineSegment | null;
  frameRef: React.RefObject<Uint8Array | null>;
  nativeFrame?: NativeFrame | null;
  pending?: boolean;
}

export function AnalysisPanel({
  showHistogram,
  showLineTool,
  lineSegment,
  frameRef,
  nativeFrame,
  pending,
}: AnalysisPanelProps) {
  const visible = showHistogram || showLineTool;

  return (
    <div className={`iv-analysis-panel${visible ? " iv-analysis-panel--open" : ""}`}>
      {visible && (
        <div className="iv-analysis-panel__content">
          {!nativeFrame && <p>{pending ? "Waiting for the matching frozen preview." : "Freeze the preview to analyze the camera’s native values."}</p>}
          {showLineTool && (
            <div className="iv-analysis-panel__slot">
              <LineProfilePanel
                frameRef={frameRef}
                lineSegment={lineSegment}
                visible={showLineTool && nativeFrame != null}
                nativeFrame={nativeFrame}
              />
            </div>
          )}
          {showHistogram && (
            <div className="iv-analysis-panel__slot">
              <HistogramOverlay
                frameRef={frameRef}
                visible={showHistogram && nativeFrame != null}
                nativeFrame={nativeFrame}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
