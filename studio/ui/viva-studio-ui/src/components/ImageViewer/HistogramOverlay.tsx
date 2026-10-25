import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  computeHistogram,
  formatHistogramStats,
  histogramStats,
  renderHistogram,
  type HistogramStats,
} from "./histogramUtils";
import { nativeHistogram, type NativeFrame } from "./nativeFrame";

const REFRESH_MS = 200; // ~5 fps refresh for histogram

interface HistogramOverlayProps {
  frameRef: React.RefObject<Uint8Array | null>;
  visible: boolean;
  nativeFrame?: NativeFrame | null;
}

export function HistogramOverlay({ frameRef, visible, nativeFrame }: HistogramOverlayProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [stats, setStats] = useState<HistogramStats | null>(null);
  const redraw = useRef<(() => void) | null>(null);
  useLayoutEffect(() => { redraw.current = null; }, [nativeFrame]);
  const analysis = useMemo(() => {
    if (!nativeFrame) return null;
    const data = nativeHistogram(nativeFrame);
    return { data, stats: histogramStats(data) };
  }, [nativeFrame]);
  const shownStats = analysis ? analysis.stats : stats;

  // Track container size and resize canvas accordingly
  useEffect(() => {
    if (!visible) return;
    const wrap = wrapRef.current;
    const canvas = canvasRef.current;
    if (!wrap || !canvas) return;

    const updateSize = () => {
      const w = Math.round(wrap.clientWidth);
      const h = Math.round(wrap.clientHeight);
      if (w > 0 && h > 0) {
        canvas.width = w;
        canvas.height = h;
        redraw.current?.();
      }
    };

    const observer = new ResizeObserver(updateSize);
    observer.observe(wrap);
    updateSize();
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible) return;

    const tick = () => {
      const bytes = frameRef.current;
      if (!nativeFrame && !bytes) return;
      const data = analysis ? analysis.data : computeHistogram(bytes!);
      if (!data) return;
      if (!analysis) setStats(histogramStats(data));
      const canvas = canvasRef.current;
      if (!canvas || canvas.width === 0 || canvas.height === 0) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      renderHistogram(ctx, data, canvas.width, canvas.height);
    };

    redraw.current = tick;
    tick();
    if (nativeFrame) return;
    const id = setInterval(tick, REFRESH_MS);
    return () => clearInterval(id);
  }, [visible, frameRef, nativeFrame, analysis]);

  if (!visible) return null;

  return (
    <div className="iv-histogram-panel">
      <div ref={wrapRef} className="iv-histogram" aria-label="Histogram">
        <canvas ref={canvasRef} className="iv-histogram__canvas" />
      </div>
      <div className="iv-histogram__axis" aria-hidden="true">
        <span>0</span>
        <span>{nativeFrame ? 2 ** nativeFrame.bits - 1 : 255}</span>
      </div>
      {shownStats && <div className="iv-histogram__stats">{formatHistogramStats(shownStats, nativeFrame ? 2 ** nativeFrame.bits - 1 : 255)}</div>}
    </div>
  );
}
