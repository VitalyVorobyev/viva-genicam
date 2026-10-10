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
  const fit = fitContain(imgW, imgH, boxW, boxH);
  return {
    backing: { width: imgW, height: imgH },
    css: { width: fit.width, height: fit.height },
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

/** Parameters for converting a wrap-relative mouse position to image-space pixel coordinates. */
export interface MouseToImageCoordsParams {
  mouseX: number;
  mouseY: number; // wrap-relative
  boxW: number;
  boxH: number;
  imgW: number;
  imgH: number;
  scale: number;
  fitScale: number;
  panX: number;
  panY: number;
}

/** Image-space pixel coordinates (integer, zero-based). */
export interface ImageCoords {
  x: number;
  y: number;
}

/**
 * Convert a wrap-relative mouse position to image-space pixel coordinates.
 *
 * The canvas is center-anchored (CSS left:50%; top:50%) and scaled by scale/fitScale
 * relative to the fit-to-contain size. The canvas itself is drawn at fitScale of the
 * original image dimensions. Inversion:
 *   imageX = (mouseX - boxW/2 - panX) / (scale/fitScale) / fitScale + imgW/2
 *
 * Returns null when the coordinates fall outside the image bounds or inputs are invalid.
 */
export function mouseToImageCoords(
  p: MouseToImageCoordsParams,
): ImageCoords | null {
  if (p.fitScale <= 0 || p.imgW <= 0 || p.imgH <= 0) return null;
  const s = p.scale / p.fitScale;
  const pixX = Math.floor(
    ((p.mouseX - p.boxW / 2 - p.panX) / s / p.fitScale) + p.imgW / 2,
  );
  const pixY = Math.floor(
    ((p.mouseY - p.boxH / 2 - p.panY) / s / p.fitScale) + p.imgH / 2,
  );
  if (pixX < 0 || pixX >= p.imgW || pixY < 0 || pixY >= p.imgH) return null;
  return { x: pixX, y: pixY };
}

/**
 * Variant of `mouseToImageCoords` that clamps out-of-bounds coordinates to the
 * image edges instead of returning null.
 *
 * Useful for ROI selection where the user may drag beyond the image boundary —
 * the selection is clamped to the nearest valid pixel rather than being discarded.
 *
 * Returns null only when the transform parameters are invalid (fitScale ≤ 0 or
 * zero image dimensions).
 */
export function mouseToImageCoordsClamped(
  p: MouseToImageCoordsParams,
): ImageCoords | null {
  if (p.fitScale <= 0 || p.imgW <= 0 || p.imgH <= 0) return null;
  const s = p.scale / p.fitScale;
  const pixX = Math.floor(
    ((p.mouseX - p.boxW / 2 - p.panX) / s / p.fitScale) + p.imgW / 2,
  );
  const pixY = Math.floor(
    ((p.mouseY - p.boxH / 2 - p.panY) / s / p.fitScale) + p.imgH / 2,
  );
  return {
    x: Math.max(0, Math.min(p.imgW - 1, pixX)),
    y: Math.max(0, Math.min(p.imgH - 1, pixY)),
  };
}
