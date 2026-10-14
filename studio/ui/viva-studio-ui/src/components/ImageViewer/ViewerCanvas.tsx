import React, { useEffect, useEffectEvent, useRef, useState } from "react";
import {
  canvasLayout,
  imageRendering,
  observedBoxSize,
  clampedPixel,
  mouseToImageCoords,
  wrapToImage,
} from "./viewerUtils";
import type { ImageSpan, Point, ViewTransform } from "./viewerUtils";
import { useZoomPan } from "./useZoomPan";
import type { ZoomPanState } from "./useZoomPan";
import { buildTransform } from "./zoomPanUtils";
import { samplePixel, formatPixelSample } from "./pixelInspectorUtils";
import type { PixelHoverInfo } from "./pixelInspectorUtils";
import { CrosshairOverlay } from "./CrosshairOverlay";
import { RoiOverlay } from "./RoiOverlay";
import { buildImageRect, clampRoiToImage } from "./roiUtils";
import type { ImageRect } from "./roiUtils";
import { LineOverlay } from "./LineOverlay";
import type { LineSegment } from "./lineProfileUtils";

export type { PixelHoverInfo };

export interface StreamInfoFrame {
  type: "info";
  width: number;
  height: number;
  pixel_format: string;
  encoding: string;
}

interface ViewerCanvasProps {
  wsUrl: string;
  onFrameStats: (fps: number) => void;
  onFrame?: () => void;
  onZoomPanChange?: (state: ZoomPanState) => void;
  pixelFormat?: string;
  onPixelHover?: (info: PixelHoverInfo | null) => void;
  resetZoomRef?: React.RefObject<(() => void) | null>;
  isStreaming?: boolean;
  snapshotRef?: React.RefObject<Uint8Array | null>;
  onStreamInfoChange?: (info: StreamInfoFrame) => void;
  zoomTo100Ref?: React.RefObject<(() => void) | null>;
  roiMode?: boolean;
  onRoiSelect?: (roi: ImageRect | null) => void;
  lineMode?: boolean;
  onLineSelect?: (seg: LineSegment | null) => void;
  /** False while another tab is shown: frames are kept but not drawn. */
  active?: boolean;
}

export function ViewerCanvas({
  wsUrl,
  onFrameStats,
  onFrame,
  onZoomPanChange,
  pixelFormat,
  onPixelHover,
  resetZoomRef,
  isStreaming,
  snapshotRef,
  onStreamInfoChange,
  zoomTo100Ref,
  roiMode,
  onRoiSelect,
  lineMode,
  onLineSelect,
  active = true,
}: ViewerCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const lastFrameRef = useRef<Uint8Array | null>(null);
  // Whether a frame has been drawn; shown as "Acquisition stopped" once the
  // stream ends.
  const [hasRenderedFrame, setHasRenderedFrame] = useState(false);

  const isActive = useEffectEvent(() => active);
  const [imgSize, setImgSize] = useState({ w: 0, h: 0 });
  const [boxSize, setBoxSize] = useState({ w: 0, h: 0 });
  const [wrapMouse, setWrapMouse] = useState<{ x: number; y: number } | null>(
    null,
  );

  // The ROI and line overlays are kept in image pixels, not wrap pixels, and
  // drawn through the current view: a zoom or a pan moves them with the image.

  // ── ROI drag state ───────────────────────────────────────────────────────────
  const roiDragRef = useRef<{ active: boolean; start: Point }>({
    active: false,
    start: { x: 0, y: 0 },
  });
  const [roiOverlayRect, setRoiOverlayRect] = useState<ImageSpan | null>(null);

  // Clear the overlay when ROI mode is turned off, while rendering rather
  // than in an effect that would render twice.
  const [prevRoiMode, setPrevRoiMode] = useState(roiMode);
  if (roiMode !== prevRoiMode) {
    setPrevRoiMode(roiMode);
    if (!roiMode) setRoiOverlayRect(null);
  }

  // ── Line drag state ──────────────────────────────────────────────────────────
  const lineDragRef = useRef<{ active: boolean; start: Point }>({
    active: false,
    start: { x: 0, y: 0 },
  });
  const [lineOverlay, setLineOverlay] = useState<ImageSpan | null>(null);

  // Clear the overlay when line mode is turned off.
  const [prevLineMode, setPrevLineMode] = useState(lineMode);
  if (lineMode !== prevLineMode) {
    setPrevLineMode(lineMode);
    if (!lineMode) setLineOverlay(null);
  }

  // Track box size via ResizeObserver
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;

    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { width, height } = entry.contentRect;
      setBoxSize((previous) => observedBoxSize(previous, width, height));
    });
    observer.observe(wrap);

    // Seed initial size immediately
    setBoxSize((previous) => observedBoxSize(previous, wrap.clientWidth, wrap.clientHeight));

    return () => observer.disconnect();
  }, []);

  const zoomPan = useZoomPan(imgSize.w, imgSize.h, boxSize.w, boxSize.h);

  // Attach native wheel listener (passive: false so we can preventDefault)
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const handler = zoomPan.getWheelHandler(wrap);
    wrap.addEventListener("wheel", handler, { passive: false });
    return () => wrap.removeEventListener("wheel", handler);
    // Re-attach when imgSize/boxSize change so the handler captures fresh dims
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imgSize.w, imgSize.h, boxSize.w, boxSize.h]);

  // Keep the canvas at the fit size when the viewer resizes between frames.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || imgSize.w === 0) return;
    const { css } = canvasLayout(imgSize.w, imgSize.h, boxSize.w, boxSize.h);
    canvas.style.width = `${css.width}px`;
    canvas.style.height = `${css.height}px`;
  }, [imgSize.w, imgSize.h, boxSize.w, boxSize.h]);

  // Apply CSS transform whenever zoom/pan state changes
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { scale, panX, panY, fitScale } = zoomPan.state;
    canvas.style.transform = buildTransform(panX, panY, scale, fitScale);
    canvas.style.imageRendering = imageRendering(scale);
  }, [zoomPan.state]);

  // Emit zoom/pan state to parent
  useEffect(() => {
    onZoomPanChange?.(zoomPan.state);
  }, [zoomPan.state, onZoomPanChange]);

  // Expose reset-zoom callback to parent via ref
  useEffect(() => {
    if (!resetZoomRef) return;
    resetZoomRef.current = zoomPan.onDoubleClick;
    return () => {
      resetZoomRef.current = null;
    };
  }, [resetZoomRef, zoomPan.onDoubleClick]);

  // Expose zoom-to-100% callback to parent via ref
  useEffect(() => {
    if (!zoomTo100Ref) return;
    zoomTo100Ref.current = zoomPan.zoomTo100;
    return () => {
      zoomTo100Ref.current = null;
    };
  }, [zoomTo100Ref, zoomPan.zoomTo100]);

  const emitFrameStats = useEffectEvent((fps: number) => {
    onFrameStats(fps);
  });

  const emitFrame = useEffectEvent(() => {
    onFrame?.();
  });

  const emitStreamInfoChange = useEffectEvent((info: StreamInfoFrame) => {
    onStreamInfoChange?.(info);
  });

  // WebSocket + render loop
  useEffect(() => {
    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";

    const frameTimes: number[] = [];
    let firstInfoLogged = false;
    let firstBinaryLogged = false;
    let disposed = false;

    ws.onopen = () => {
      console.info("[ViewerCanvas] WebSocket opened", { wsUrl });
    };

    ws.onerror = (event) => {
      console.error("[ViewerCanvas] WebSocket error", { wsUrl, event });
      if (!disposed) {
        emitFrameStats(0);
      }
    };

    ws.onclose = (event) => {
      console.info("[ViewerCanvas] WebSocket closed", {
        wsUrl,
        code: event.code,
        reason: event.reason,
        wasClean: event.wasClean,
      });
      if (!disposed) {
        emitFrameStats(0);
      }
    };

    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        if (!firstInfoLogged) {
          console.info("[ViewerCanvas] First stream info frame", {
            wsUrl,
            payload: event.data,
          });
          firstInfoLogged = true;
        }
        try {
          const parsed = JSON.parse(event.data) as StreamInfoFrame;
          if (parsed.type === "info") {
            emitStreamInfoChange(parsed);
          }
        } catch {
          // ignore malformed text messages
        }
        return;
      }

      // Store raw bytes for pixel sampling before decoding
      lastFrameRef.current = new Uint8Array(event.data);
      if (snapshotRef) snapshotRef.current = lastFrameRef.current;

      if (!firstBinaryLogged) {
        console.info("[ViewerCanvas] First binary frame received", {
          wsUrl,
          bytes: lastFrameRef.current.length,
        });
        firstBinaryLogged = true;
      }

      // Hidden behind another tab: keep the frame for the analysis tools, but
      // decoding and drawing it would be wasted.
      if (!isActive()) return;

      const blob = new Blob([event.data]);
      createImageBitmap(blob)
        .then((bitmap) => {
          const canvas = canvasRef.current;
          const wrap = wrapRef.current;
          if (disposed || !canvas || !wrap) {
            bitmap.close();
            return;
          }

          const newImgW = bitmap.width;
          const newImgH = bitmap.height;

          // Every camera pixel in the backing store, shown at the fit size.
          // Resizing the backing store clears it, so only on a change.
          const layout = canvasLayout(newImgW, newImgH, wrap.clientWidth, wrap.clientHeight);
          if (canvas.width !== layout.backing.width) canvas.width = layout.backing.width;
          if (canvas.height !== layout.backing.height) canvas.height = layout.backing.height;
          canvas.style.width = `${layout.css.width}px`;
          canvas.style.height = `${layout.css.height}px`;

          // Centre the canvas using CSS; transform is managed by zoom/pan effect
          canvas.style.left = "50%";
          canvas.style.top = "50%";
          canvas.style.transformOrigin = "center center";

          const ctx = canvas.getContext("2d");
          if (ctx) {
            ctx.drawImage(bitmap, 0, 0);
          }
          bitmap.close();
          setHasRenderedFrame(true);

          // Update imgSize state only when dimensions actually change
          setImgSize((prev) => {
            if (prev.w !== newImgW || prev.h !== newImgH) {
              return { w: newImgW, h: newImgH };
            }
            return prev;
          });

          // Rolling 1-second FPS window
          const now = performance.now();
          frameTimes.push(now);
          while (frameTimes.length > 0 && now - frameTimes[0] > 1000) {
            frameTimes.shift();
          }
          emitFrameStats(frameTimes.length);
          emitFrame();
        })
        .catch(() => {
          // Ignore individual frame decode errors
        });
    };

    return () => {
      disposed = true;
      ws.close();
    };
  }, [wsUrl, snapshotRef]);

  // rAF-throttled mouse move handler to avoid 60Hz React re-renders
  const rafPendingRef = useRef(false);
  const lastMouseRef = useRef<{ x: number; y: number } | null>(null);

  const handleMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    lastMouseRef.current = { x, y };

    if (rafPendingRef.current) return;
    rafPendingRef.current = true;

    requestAnimationFrame(() => {
      rafPendingRef.current = false;
      const pos = lastMouseRef.current;
      if (!pos) return;

      setWrapMouse(pos);

      if (!onPixelHover) return;

      const { scale, panX, panY, fitScale } = zoomPan.state;
      const coords = mouseToImageCoords({
        mouseX: pos.x,
        mouseY: pos.y,
        boxW: boxSize.w,
        boxH: boxSize.h,
        imgW: imgSize.w,
        imgH: imgSize.h,
        scale,
        fitScale,
        panX,
        panY,
      });

      if (!coords) {
        onPixelHover(null);
        return;
      }

      const frame = lastFrameRef.current;
      const fmt = pixelFormat ?? "Mono8";
      const sample =
        frame
          ? samplePixel(frame, coords.x, coords.y, imgSize.w, imgSize.h, fmt)
          : null;
      const formatted = sample ? formatPixelSample(sample) : "";
      onPixelHover({ coords, sample, formatted });
    });
  };

  const handleMouseLeave = () => {
    setWrapMouse(null);
    onPixelHover?.(null);
  };

  // The view the overlays and pointer positions are mapped through.
  const view: ViewTransform = {
    boxW: boxSize.w,
    boxH: boxSize.h,
    imgW: imgSize.w,
    imgH: imgSize.h,
    scale: zoomPan.state.scale,
    fitScale: zoomPan.state.fitScale,
    panX: zoomPan.state.panX,
    panY: zoomPan.state.panY,
  };

  /** The image point under a pointer event, or `null` before the first frame. */
  const imagePointOf = (e: React.PointerEvent<HTMLDivElement>): Point | null => {
    const bRect = e.currentTarget.getBoundingClientRect();
    return wrapToImage(view, { x: e.clientX - bRect.left, y: e.clientY - bRect.top });
  };

  const pixelOf = (p: Point) => clampedPixel(p, imgSize.w, imgSize.h);

  // ── ROI pointer handlers ─────────────────────────────────────────────────────

  const handleRoiPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const at = imagePointOf(e);
    if (!at) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    roiDragRef.current = { active: true, start: at };
    setRoiOverlayRect({ from: at, to: at });
    onRoiSelect?.(null);
  };

  const handleRoiPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!roiDragRef.current.active) return;
    const at = imagePointOf(e);
    if (!at) return;
    setRoiOverlayRect({ from: roiDragRef.current.start, to: at });
  };

  const handleRoiPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!roiDragRef.current.active) return;
    roiDragRef.current.active = false;
    const at = imagePointOf(e);
    if (!at) return;

    const raw = buildImageRect(pixelOf(roiDragRef.current.start), pixelOf(at));
    const roi = clampRoiToImage(raw, imgSize.w, imgSize.h);
    // What stays drawn is what Apply writes.
    setRoiOverlayRect({ from: { x: roi.x, y: roi.y }, to: { x: roi.x + roi.w, y: roi.y + roi.h } });
    onRoiSelect?.(roi);
  };

  // ── Line pointer handlers ────────────────────────────────────────────────────

  const handleLinePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const at = imagePointOf(e);
    if (!at) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    lineDragRef.current = { active: true, start: at };
    setLineOverlay({ from: at, to: at });
    onLineSelect?.(null);
  };

  const handleLinePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!lineDragRef.current.active) return;
    const at = imagePointOf(e);
    if (!at) return;
    setLineOverlay({ from: lineDragRef.current.start, to: at });
  };

  const handleLinePointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!lineDragRef.current.active) return;
    lineDragRef.current.active = false;
    const at = imagePointOf(e);
    if (!at) return;

    const start = pixelOf(lineDragRef.current.start);
    const end = pixelOf(at);
    // What stays drawn joins the centres of the pixels the profile samples.
    setLineOverlay({
      from: { x: start.x + 0.5, y: start.y + 0.5 },
      to: { x: end.x + 0.5, y: end.y + 0.5 },
    });
    onLineSelect?.({ x0: start.x, y0: start.y, x1: end.x, y1: end.y });
  };

  // ── Cursor and class derivation ─────────────────────────────────────────────

  const isZoomed = imgSize.w > 0 && zoomPan.state.scale > zoomPan.state.fitScale;
  const isDragging = zoomPan.isDraggingRef.current;
  let cursorClass = "";
  if (lineMode || roiMode) {
    cursorClass = "iv-canvas-wrap--crosshair";
  } else if (isZoomed) {
    cursorClass = isDragging ? "iv-canvas-wrap--grabbing" : "iv-canvas-wrap--grab";
  }
  const streamClass = isStreaming ? "iv-canvas-wrap--active" : "iv-canvas-wrap--idle";

  // Choose pointer handlers: lineMode > roiMode > zoom/pan
  const pointerHandlers = lineMode
    ? {
        onPointerDown: handleLinePointerDown,
        onPointerMove: handleLinePointerMove,
        onPointerUp: handleLinePointerUp,
        onDoubleClick: undefined,
      }
    : roiMode
    ? {
        onPointerDown: handleRoiPointerDown,
        onPointerMove: handleRoiPointerMove,
        onPointerUp: handleRoiPointerUp,
        onDoubleClick: undefined,
      }
    : {
        onPointerDown: zoomPan.onPointerDown,
        onPointerMove: zoomPan.onPointerMove,
        onPointerUp: zoomPan.onPointerUp,
        onDoubleClick: zoomPan.onDoubleClick,
      };

  const suppressCrosshair = lineMode || roiMode;

  return (
    <div
      ref={wrapRef}
      className={`iv-canvas-wrap ${streamClass}${cursorClass ? ` ${cursorClass}` : ""}`}
      {...pointerHandlers}
      onMouseMove={handleMouseMove}
      onMouseLeave={handleMouseLeave}
    >
      <canvas ref={canvasRef} />
      <CrosshairOverlay
        mouseX={suppressCrosshair ? null : (wrapMouse?.x ?? null)}
        mouseY={suppressCrosshair ? null : (wrapMouse?.y ?? null)}
      />
      <RoiOverlay rect={roiMode ? roiOverlayRect : null} view={view} />
      <LineOverlay line={lineMode ? lineOverlay : null} view={view} />
      {!isStreaming && hasRenderedFrame && (
        <div className="iv-canvas-overlay">Acquisition stopped</div>
      )}
      {/* Histogram and line profile moved to AnalysisPanel dock */}
    </div>
  );
}
