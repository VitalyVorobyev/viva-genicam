/**
 * Line profile utilities for the Image Viewer.
 *
 * Implements Bresenham's line algorithm for pixel-accurate line traversal,
 * BMP-aware pixel sampling along the line, and canvas rendering of the
 * resulting intensity profile.
 *
 * Supported BMP formats (same as pixelInspectorUtils / histogramUtils):
 *  - 8bpp grayscale → ProfileData { kind: "mono" }
 *  - 24bpp RGB      → ProfileData { kind: "rgb" }
 */

/** Image-space line segment defined by two integer pixel endpoints. */
export interface LineSegment {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** Intensity values sampled along a line segment. */
export type ProfileData =
  | { kind: "mono"; values: Float32Array; length: number }
  | { kind: "rgb"; r: Float32Array; g: Float32Array; b: Float32Array; length: number };

// ── BMP header helpers (private, same pattern as histogramUtils) ──────────────

function readU32LE(buf: Uint8Array, offset: number): number {
  return (
    (buf[offset]! |
      (buf[offset + 1]! << 8) |
      (buf[offset + 2]! << 16) |
      (buf[offset + 3]! << 24)) >>>
    0
  );
}

function readI32LE(buf: Uint8Array, offset: number): number {
  const view = new DataView(buf.buffer, buf.byteOffset + offset, 4);
  return view.getInt32(0, true);
}

function readU16LE(buf: Uint8Array, offset: number): number {
  return buf[offset]! | (buf[offset + 1]! << 8);
}

// ── Pure functions ─────────────────────────────────────────────────────────────

/**
 * Bresenham's line algorithm.
 *
 * Returns the ordered sequence of integer pixel coordinates on the raster path
 * from (x0, y0) to (x1, y1), inclusive. Always includes both endpoints.
 * Returns a single element when start equals end.
 *
 * @param x0  Start x (integer).
 * @param y0  Start y (integer).
 * @param x1  End x (integer).
 * @param y1  End y (integer).
 */
export function bresenhamLine(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
): Array<{ x: number; y: number }> {
  const pixels: Array<{ x: number; y: number }> = [];
  let x = Math.round(x0);
  let y = Math.round(y0);
  const ex = Math.round(x1);
  const ey = Math.round(y1);

  const dx = Math.abs(ex - x);
  const dy = Math.abs(ey - y);
  const sx = x < ex ? 1 : -1;
  const sy = y < ey ? 1 : -1;
  let err = dx - dy;

  while (true) {
    pixels.push({ x, y });
    if (x === ex && y === ey) break;
    const e2 = 2 * err;
    if (e2 > -dy) {
      err -= dy;
      x += sx;
    }
    if (e2 < dx) {
      err += dx;
      y += sy;
    }
  }

  return pixels;
}

/**
 * Sample pixel intensities along a line segment from a BMP-encoded frame.
 *
 * Parses the BMP file header to determine pixel data offset, dimensions, and
 * bit depth. Walks the Bresenham raster path between (x0,y0) and (x1,y1)
 * in image-pixel coordinates, sampling each pixel.
 *
 * Returns null when:
 *  - Buffer shorter than 54 bytes (no valid BMP header)
 *  - Unsupported bit depth (anything other than 8 or 24)
 *  - Zero-area image
 *
 * Input coordinates are clamped to image bounds before line traversal.
 *
 * @param bmpBytes  Full BMP file bytes (from WebSocket binary frame).
 * @param x0        Line start x in image pixels.
 * @param y0        Line start y in image pixels.
 * @param x1        Line end x in image pixels.
 * @param y1        Line end y in image pixels.
 */
export function sampleLineProfile(
  bmpBytes: Uint8Array,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
): ProfileData | null {
  if (bmpBytes.length < 54) return null;

  const dataOffset = readU32LE(bmpBytes, 10);
  const imgW = Math.abs(readI32LE(bmpBytes, 18));
  const imgH = Math.abs(readI32LE(bmpBytes, 22));
  const bitsPerPixel = readU16LE(bmpBytes, 28);

  if (imgW === 0 || imgH === 0) return null;

  // Clamp endpoints to image bounds
  const cx0 = Math.max(0, Math.min(Math.round(x0), imgW - 1));
  const cy0 = Math.max(0, Math.min(Math.round(y0), imgH - 1));
  const cx1 = Math.max(0, Math.min(Math.round(x1), imgW - 1));
  const cy1 = Math.max(0, Math.min(Math.round(y1), imgH - 1));

  const pixels = bresenhamLine(cx0, cy0, cx1, cy1);
  const len = pixels.length;

  if (bitsPerPixel === 8) {
    const rowStride = (imgW + 3) & ~3;
    const values = new Float32Array(len);
    for (let i = 0; i < len; i++) {
      const p = pixels[i]!;
      const idx = dataOffset + p.y * rowStride + p.x;
      values[i] = idx < bmpBytes.length ? bmpBytes[idx]! : 0;
    }
    return { kind: "mono", values, length: len };
  }

  if (bitsPerPixel === 24) {
    const rowStride = (imgW * 3 + 3) & ~3;
    const r = new Float32Array(len);
    const g = new Float32Array(len);
    const b = new Float32Array(len);
    for (let i = 0; i < len; i++) {
      const p = pixels[i]!;
      const base = dataOffset + p.y * rowStride + p.x * 3;
      if (base + 2 < bmpBytes.length) {
        b[i] = bmpBytes[base]!;       // BMP stores BGR
        g[i] = bmpBytes[base + 1]!;
        r[i] = bmpBytes[base + 2]!;
      }
    }
    return { kind: "rgb", r, g, b, length: len };
  }

  // Unsupported bit depth.
  return null;
}

/**
 * Render a line intensity profile onto a 2D canvas context as a polyline.
 *
 * Values are normalised from 0–maximum to canvas height. When the profile has
 * fewer than 2 points the canvas is cleared but nothing is drawn.
 *
 * - Mono: single white/gray polyline.
 * - RGB: three polylines in semi-transparent R, G, B.
 *
 * @param ctx   2D rendering context of the profile canvas.
 * @param data  Profile data as returned by `sampleLineProfile`.
 * @param w     Canvas width in pixels.
 * @param h     Canvas height in pixels.
 */
export function renderLineProfile(
  ctx: CanvasRenderingContext2D,
  data: ProfileData,
  w: number,
  h: number,
  maximum = 255,
): void {
  ctx.clearRect(0, 0, w, h);
  if (data.length < 2) return;

  const xStep = w / (data.length - 1);

  function drawPolyline(values: Float32Array, color: string) {
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i < values.length; i++) {
      const px = i * xStep;
      const py = h - (values[i]! / maximum) * h;
      if (i === 0) {
        ctx.moveTo(px, py);
      } else {
        ctx.lineTo(px, py);
      }
    }
    ctx.stroke();
  }

  if (data.kind === "mono") {
    drawPolyline(data.values, "rgba(230, 237, 243, 0.9)");
  } else {
    drawPolyline(data.r, "rgba(248, 81, 73, 0.8)");
    drawPolyline(data.g, "rgba(63, 185, 80, 0.8)");
    drawPolyline(data.b, "rgba(47, 129, 247, 0.8)");
  }
}
