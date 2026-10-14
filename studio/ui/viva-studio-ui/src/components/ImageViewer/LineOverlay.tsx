import { imageToWrap } from "./viewerUtils";
import type { ImageSpan, ViewTransform } from "./viewerUtils";

interface LineOverlayProps {
  /** The line's ends, in image pixels. */
  line: ImageSpan | null;
  /** The current zoom and pan, through which the line is drawn. */
  view: ViewTransform;
}

/**
 * Absolutely-positioned SVG overlay that renders the current line profile
 * selection on top of the image canvas.
 *
 * The line is kept in image pixels and projected through the view on every
 * render, so it stays on the same pixels as the image is zoomed and panned.
 * Draws the line segment plus circular endpoint markers.
 * pointer-events: none — does not interfere with other interactions.
 */
export function LineOverlay({ line, view }: LineOverlayProps) {
  if (!line) return null;
  const a = imageToWrap(view, line.from);
  const b = imageToWrap(view, line.to);
  if (!a || !b) return null;

  return (
    <svg
      className="iv-line-overlay"
      style={{
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
        overflow: "visible",
        width: "100%",
        height: "100%",
      }}
    >
      {/* Line segment */}
      <line
        x1={a.x}
        y1={a.y}
        x2={b.x}
        y2={b.y}
        stroke="var(--warning)"
        strokeWidth="1.5"
        strokeDasharray="6 3"
        opacity="0.9"
      />
      {/* Start endpoint dot */}
      <circle cx={a.x} cy={a.y} r={4} fill="var(--warning)" opacity="0.9" />
      {/* End endpoint dot */}
      <circle cx={b.x} cy={b.y} r={4} fill="var(--warning)" opacity="0.9" />
    </svg>
  );
}
