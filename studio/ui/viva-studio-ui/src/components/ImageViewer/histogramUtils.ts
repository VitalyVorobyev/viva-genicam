/**
 * Histogram computation and rendering utilities for the Image Viewer.
 *
 * Reads pixel data directly from BMP-encoded frame bytes (as delivered by
 * the app's streamer). The BMP header is parsed to determine image dimensions
 * and bit depth, so no external metadata is required.
 *
 * Supported BMP formats:
 *  - 8bpp grayscale → HistogramData { kind: "mono" }
 *  - 24bpp RGB      → HistogramData { kind: "rgb" }
 */

export type HistogramData =
  | { kind: "mono"; values: Uint32Array }
  | { kind: "rgb"; r: Uint32Array; g: Uint32Array; b: Uint32Array };

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

function arrayMax(arr: Uint32Array): number {
  let max = 0;
  for (let i = 0; i < arr.length; i++) {
    if (arr[i]! > max) max = arr[i]!;
  }
  return max;
}

/**
 * Compute a 256-bin histogram from a BMP-encoded frame buffer.
 *
 * Parses the BMP file header to determine pixel data offset, image dimensions,
 * and bit depth. Returns null for buffers shorter than 54 bytes or for
 * unsupported bit depths (anything other than 8 or 24).
 *
 * @param bmpBytes  Full BMP file bytes (from WebSocket binary frame).
 * @returns HistogramData or null when the buffer is invalid / unsupported.
 */
export function computeHistogram(bmpBytes: Uint8Array): HistogramData | null {
  // Minimum valid BMP: 14-byte file header + 40-byte BITMAPINFOHEADER.
  if (bmpBytes.length < 54) return null;

  const dataOffset = readU32LE(bmpBytes, 10);
  const imgW = readI32LE(bmpBytes, 18); // may be negative (unusual but valid)
  const imgH = readI32LE(bmpBytes, 22); // negative = top-down BMP
  const bitsPerPixel = readU16LE(bmpBytes, 28);

  const w = Math.abs(imgW);
  const h = Math.abs(imgH);
  if (w === 0 || h === 0) return null;

  if (bitsPerPixel === 8) {
    const values = new Uint32Array(256);
    const rowStride = (w + 3) & ~3;
    for (let y = 0; y < h; y++) {
      const rowBase = dataOffset + y * rowStride;
      for (let x = 0; x < w; x++) {
        const idx = rowBase + x;
        if (idx >= bmpBytes.length) break;
        values[bmpBytes[idx]!]!++;
      }
    }
    return { kind: "mono", values };
  }

  if (bitsPerPixel === 24) {
    const r = new Uint32Array(256);
    const g = new Uint32Array(256);
    const b = new Uint32Array(256);
    const rowStride = (w * 3 + 3) & ~3;
    for (let y = 0; y < h; y++) {
      const rowBase = dataOffset + y * rowStride;
      for (let x = 0; x < w; x++) {
        const base = rowBase + x * 3;
        if (base + 2 >= bmpBytes.length) break;
        // BMP stores BGR order.
        b[bmpBytes[base]!]!++;
        g[bmpBytes[base + 1]!]!++;
        r[bmpBytes[base + 2]!]!++;
      }
    }
    return { kind: "rgb", r, g, b };
  }

  // Unsupported bit depth.
  return null;
}

/** Summary of a histogram at its source bit depth. */
export interface HistogramStats {
  min: number;
  max: number;
  mean: number;
  /** Share of samples at the highest representable level, which may have clipped. */
  saturated: number;
}

/**
 * Range, mean and saturated share of a histogram. A colour
 * image pools the samples of its three channels. `null` when it is empty.
 */
export function histogramStats(data: HistogramData): HistogramStats | null {
  const channels = data.kind === "mono" ? [data.values] : [data.r, data.g, data.b];
  let count = 0;
  let sum = 0;
  let min = Number.POSITIVE_INFINITY;
  let max = -1;
  for (const bins of channels) {
    bins.forEach((n, level) => {
      if (n === 0) return;
      count += n;
      sum += n * level;
      min = Math.min(min, level);
      max = Math.max(max, level);
    });
  }
  if (count === 0) return null;
  const saturated = channels.reduce((total, bins) => total + bins[bins.length - 1], 0) / count;
  return { min, max, mean: sum / count, saturated };
}

/** One line of histogram numbers; the clipped share only when some clipped. */
export function formatHistogramStats({ min, max, mean, saturated }: HistogramStats, maximum = 255): string {
  const line = `min ${min} · max ${max} · mean ${mean.toFixed(1)}`;
  return saturated > 0 ? `${line} · ${(saturated * 100).toFixed(1)}% at ${maximum}` : line;
}

/**
 * Render histogram data onto a 2D canvas context.
 *
 * - Mono: white bars, peak-normalised.
 * - RGB: semi-transparent red, green, blue bars drawn with additive blending
 *        (each channel is independently normalised to the overall peak).
 *
 * @param ctx   2D rendering context of the histogram canvas.
 * @param data  Histogram data as returned by `computeHistogram`.
 * @param w     Canvas width in pixels.
 * @param h     Canvas height in pixels.
 */
export function renderHistogram(
  ctx: CanvasRenderingContext2D,
  data: HistogramData,
  w: number,
  h: number,
): void {
  ctx.clearRect(0, 0, w, h);

  const bins = data.kind === "mono" ? data.values.length : data.r.length;
  const binW = w / bins;

  if (data.kind === "mono") {
    const peak = Math.max(arrayMax(data.values), 1);
    ctx.fillStyle = "rgba(230, 237, 243, 0.85)";
    for (let i = 0; i < bins; i++) {
      const barH = (data.values[i]! / peak) * h;
      ctx.fillRect(i * binW, h - barH, Math.max(binW, 1), barH);
    }
    return;
  }

  // RGB: find single peak across all channels for consistent scale.
  const peak = Math.max(arrayMax(data.r), arrayMax(data.g), arrayMax(data.b), 1);

  const channels: [Uint32Array, string][] = [
    [data.r, "rgba(248, 81, 73, 0.65)"],
    [data.g, "rgba(63, 185, 80, 0.65)"],
    [data.b, "rgba(47, 129, 247, 0.65)"],
  ];

  for (const [channel, color] of channels) {
    ctx.fillStyle = color;
    for (let i = 0; i < bins; i++) {
      const barH = (channel[i]! / peak) * h;
      ctx.fillRect(i * binW, h - barH, Math.max(binW, 1), barH);
    }
  }
}
