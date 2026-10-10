import { useEffect, useRef, useState } from "react";
import {
  computeHistogram,
  formatHistogramStats,
  histogramStats,
  renderHistogram,
  type HistogramStats,
} from "./histogramUtils";

const REFRESH_MS = 200; // ~5 fps refresh for histogram

interface HistogramOverlayProps {
  frameRef: React.RefObject<Uint8Array | null>;
  visible: boolean;
}

export function HistogramOverlay({ frameRef, visible }: HistogramOverlayProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [stats, setStats] = useState<HistogramStats | null>(null);

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
      if (!bytes) return;
      const data = computeHistogram(bytes);
      if (!data) return;
      setStats(histogramStats(data));
      const canvas = canvasRef.current;
      if (!canvas || canvas.width === 0 || canvas.height === 0) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      renderHistogram(ctx, data, canvas.width, canvas.height);
    };

    tick();
    const id = setInterval(tick, REFRESH_MS);
    return () => clearInterval(id);
  }, [visible, frameRef]);

  if (!visible) return null;

  return (
    <div className="iv-histogram-panel">
      <div ref={wrapRef} className="iv-histogram" aria-label="Histogram">
        <canvas ref={canvasRef} className="iv-histogram__canvas" />
      </div>
      <div className="iv-histogram__axis" aria-hidden="true">
        <span>0</span>
        <span>255</span>
      </div>
      {stats && <div className="iv-histogram__stats">{formatHistogramStats(stats)}</div>}
    </div>
  );
}
