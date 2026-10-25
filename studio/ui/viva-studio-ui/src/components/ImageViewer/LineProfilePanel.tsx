import { useEffect, useLayoutEffect, useRef } from "react";
import { sampleLineProfile, renderLineProfile } from "./lineProfileUtils";
import type { LineSegment } from "./lineProfileUtils";
import { nativeLineProfile, type NativeFrame } from "./nativeFrame";

const REFRESH_MS = 200; // ~5 fps refresh

interface LineProfilePanelProps {
  frameRef: React.RefObject<Uint8Array | null>;
  lineSegment: LineSegment | null;
  visible: boolean;
  nativeFrame?: NativeFrame | null;
}

export function LineProfilePanel({ frameRef, lineSegment, visible, nativeFrame }: LineProfilePanelProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const redraw = useRef<(() => void) | null>(null);
  useLayoutEffect(() => { redraw.current = null; }, [nativeFrame, lineSegment]);

  // Track container size and resize canvas accordingly
  useEffect(() => {
    if (!visible || !lineSegment) return;
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
  }, [visible, lineSegment]);

  useEffect(() => {
    if (!visible || !lineSegment) return;

    const tick = () => {
      const bytes = frameRef.current;
      if (!nativeFrame && !bytes) return;
      const data = nativeFrame ? nativeLineProfile(nativeFrame, lineSegment.x0, lineSegment.y0, lineSegment.x1, lineSegment.y1) : sampleLineProfile(
        bytes!,
        lineSegment.x0,
        lineSegment.y0,
        lineSegment.x1,
        lineSegment.y1,
      );
      if (!data) return;
      const canvas = canvasRef.current;
      if (!canvas || canvas.width === 0 || canvas.height === 0) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      renderLineProfile(ctx, data, canvas.width, canvas.height, nativeFrame ? 2 ** nativeFrame.bits - 1 : 255);
    };

    redraw.current = tick;
    tick();
    if (nativeFrame) return;
    const id = setInterval(tick, REFRESH_MS);
    return () => clearInterval(id);
  }, [visible, lineSegment, frameRef, nativeFrame]);

  if (!visible || !lineSegment) return null;

  return (
    <div ref={wrapRef} className="iv-line-profile" aria-label="Line profile">
      <canvas ref={canvasRef} className="iv-line-profile__canvas" />
    </div>
  );
}
