import { imageToWrap } from "./viewerUtils";
import type { ImageSpan, ViewTransform } from "./viewerUtils";

interface RoiOverlayProps {
  /** Two opposite corners of the selection, in image pixels. */
  rect: ImageSpan | null;
  /** The current zoom and pan, through which the selection is drawn. */
  view: ViewTransform;
}

/**
 * Absolutely-positioned SVG overlay that renders the current ROI selection
 * rectangle on top of the image canvas.
 *
 * The selection is kept in image pixels and projected through the view on
 * every render, so it stays on the same pixels as the image is zoomed and
 * panned.
 * pointer-events: none — does not interfere with zoom/pan or ROI drag handlers.
 */
export function RoiOverlay({ rect, view }: RoiOverlayProps) {
  if (!rect) return null;
  const a = imageToWrap(view, rect.from);
  const b = imageToWrap(view, rect.to);
  if (!a || !b) return null;

  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  const w = Math.abs(b.x - a.x);
  const h = Math.abs(b.y - a.y);

  return (
    <svg
      className="iv-roi-overlay"
      style={{
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
        overflow: "visible",
        width: "100%",
        height: "100%",
      }}
    >
      {/* Semi-transparent fill */}
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        fill="rgba(47, 129, 247, 0.08)"
        stroke="none"
      />
      {/* Dashed border */}
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        fill="none"
        stroke="var(--accent)"
        strokeWidth="1.5"
        strokeDasharray="5 3"
        opacity="0.9"
      />
      {/* Corner handles */}
      {[
        [x, y],
        [x + w, y],
        [x, y + h],
        [x + w, y + h],
      ].map(([cx, cy], i) => (
        <rect
          key={i}
          x={(cx ?? 0) - 3}
          y={(cy ?? 0) - 3}
          width={6}
          height={6}
          fill="var(--accent)"
          opacity="0.9"
        />
      ))}
    </svg>
  );
}
