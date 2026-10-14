/** Result of fitting an image into a container using contain-scaling. */
export interface FitContainResult {
  width: number;
  height: number;
  left: number;
  top: number;
}

/**
 * Compute letterbox-fit dimensions for an image inside a container.
 * Never upscales: scale is capped at 1.0.
 * Centers the scaled image within the container box.
 * Returns zero-sized result when either container dimension is zero.
 */
export function fitContain(
  imgW: number,
  imgH: number,
  boxW: number,
  boxH: number,
): FitContainResult {
  if (boxW <= 0 || boxH <= 0 || imgW <= 0 || imgH <= 0) {
    return { width: 0, height: 0, left: 0, top: 0 };
  }
  const scale = Math.min(boxW / imgW, boxH / imgH, 1);
  const width = Math.round(imgW * scale);
  const height = Math.round(imgH * scale);
  const left = Math.round((boxW - width) / 2);
  const top = Math.round((boxH - height) / 2);
  return { width, height, left, top };
}

/**
 * How the viewer canvas is sized. Its backing store holds every camera pixel
 * and CSS shows it at the fit size, so zooming in enlarges the camera's own
 * pixels; a backing store at the fit size made "1:1" an enlarged, downscaled
 * copy (backlog ST-39).
 */
export function canvasLayout(
  imgW: number,
  imgH: number,
  boxW: number,
  boxH: number,
): { backing: { width: number; height: number }; css: { width: number; height: number } } {
  // Unrounded: the zoom transform multiplies this size by scale / fitScale,
  // so 1:1 lands on the camera's exact size only if nothing was rounded
  // (Codex review of #178). CSS takes fractional pixels.
  const fitScale =
    imgW > 0 && imgH > 0 && boxW > 0 && boxH > 0 ? Math.min(boxW / imgW, boxH / imgH, 1) : 0;
  return {
    backing: { width: imgW, height: imgH },
    css: { width: imgW * fitScale, height: imgH * fitScale },
  };
}

/**
 * The viewer box after a measurement. A hidden viewer measures 0×0, which is
 * not a resize: taking it as one would reset the zoom whenever the user
 * switches tabs (backlog ST-39).
 */
export function observedBoxSize(
  previous: { w: number; h: number },
  width: number,
  height: number,
): { w: number; h: number } {
  if (width <= 0 || height <= 0) return previous;
  return { w: Math.round(width), h: Math.round(height) };
}

/**
 * CSS `image-rendering` at a zoom `scale` (CSS pixels per camera pixel):
 * enlarged pixels are drawn as squares, so each stays distinct; a reduced
 * image is smoothed.
 */
export function imageRendering(scale: number): "pixelated" | "auto" {
  return scale > 1 ? "pixelated" : "auto";
}

/**
 * Format a frames-per-second value to two decimal places.
 * Example: 30 → "30.00 fps"
 */
export function formatFps(fps: number): string {
  return `${fps.toFixed(2)} fps`;
}

/**
 * Format image dimensions as a human-readable string.
 * Uses the Unicode multiplication sign × (U+00D7), not letter x.
 * Example: 1920, 1080 → "1920 × 1080"
 */
export function formatResolution(w: number, h: number): string {
  return `${w} \u00d7 ${h}`;
}

/**
 * Format a frame count as a human-readable string with thousands separators.
 * Uses en-US locale for deterministic output across environments.
 * Examples: 0 → "0 frames", 1 → "1 frame", 1234 → "1,234 frames"
 */
export function formatFrameCount(n: number): string {
  const formatted = new Intl.NumberFormat("en-US").format(n);
  return n === 1 ? `${formatted} frame` : `${formatted} frames`;
}

/**
 * How the image is shown in the viewer box: its size, the box's, and the
 * zoom and pan. `scale` is CSS pixels per camera pixel; the canvas is drawn
 * at `fitScale` and enlarged by `scale / fitScale` about its centre, which
 * sits at the box centre plus the pan.
 */
export interface ViewTransform {
  boxW: number;
  boxH: number;
  imgW: number;
  imgH: number;
  scale: number;
  fitScale: number;
  panX: number;
  panY: number;
}

/** A point, in wrap-relative CSS pixels or in (fractional) image pixels. */
export interface Point {
  x: number;
  y: number;
}

/** Two image points: a line's ends, or a rectangle's opposite corners. */
export interface ImageSpan {
  from: Point;
  to: Point;
}

function isDrawable(v: ViewTransform): boolean {
  return v.fitScale > 0 && v.scale > 0 && v.imgW > 0 && v.imgH > 0;
}

/**
 * Where an image point is drawn in the wrap, under the current view. The
 * overlays keep their geometry in image pixels and draw it through this, so
 * a zoom or a pan moves them with the image. `null` before the first frame.
 */
export function imageToWrap(v: ViewTransform, p: Point): Point | null {
  if (!isDrawable(v)) return null;
  const s = v.scale / v.fitScale;
  return {
    x: v.boxW / 2 + v.panX + (p.x - v.imgW / 2) * v.fitScale * s,
    y: v.boxH / 2 + v.panY + (p.y - v.imgH / 2) * v.fitScale * s,
  };
}

/**
 * The image point under a wrap position: the inverse of {@link imageToWrap},
 * unrounded and unclamped. `null` before the first frame.
 */
export function wrapToImage(v: ViewTransform, p: Point): Point | null {
  if (!isDrawable(v)) return null;
  const s = v.scale / v.fitScale;
  return {
    x: (p.x - v.boxW / 2 - v.panX) / s / v.fitScale + v.imgW / 2,
    y: (p.y - v.boxH / 2 - v.panY) / s / v.fitScale + v.imgH / 2,
  };
}

/** Parameters for converting a wrap-relative mouse position to image-space pixel coordinates. */
export interface MouseToImageCoordsParams extends ViewTransform {
  mouseX: number;
  mouseY: number; // wrap-relative
}

/** Image-space pixel coordinates (integer, zero-based). */
export interface ImageCoords {
  x: number;
  y: number;
}

/**
 * Convert a wrap-relative mouse position to the image pixel under it, by
 * {@link wrapToImage}.
 *
 * Returns null when the coordinates fall outside the image bounds or inputs are invalid.
 */
export function mouseToImageCoords(
  p: MouseToImageCoordsParams,
): ImageCoords | null {
  const at = wrapToImage(p, { x: p.mouseX, y: p.mouseY });
  if (!at) return null;
  const pixX = Math.floor(at.x);
  const pixY = Math.floor(at.y);
  if (pixX < 0 || pixX >= p.imgW || pixY < 0 || pixY >= p.imgH) return null;
  return { x: pixX, y: pixY };
}

/**
 * The pixel holding an image point, clamped to the image's edges.
 *
 * A drag that leaves the image (an ROI, a profile line) ends on the nearest
 * pixel rather than being discarded.
 */
export function clampedPixel(p: Point, imgW: number, imgH: number): ImageCoords {
  return {
    x: Math.max(0, Math.min(imgW - 1, Math.floor(p.x))),
    y: Math.max(0, Math.min(imgH - 1, Math.floor(p.y))),
  };
}
