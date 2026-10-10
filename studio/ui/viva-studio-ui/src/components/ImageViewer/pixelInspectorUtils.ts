/**
 * Pixel sampling utilities for the Image Viewer crosshair inspector.
 *
 * The app's streamer delivers multi-format BMP frames:
 *
 * - Mono8, Mono10, Mono12, Mono16 → 8bpp grayscale BMP (Mono10-16 downscaled)
 * - BayerRG/GR/BG/GB 8/10/12/16 → 24bpp RGB BMP (bilinear debayered)
 * - RGB8, BGR8, RGBa8 → 24bpp RGB BMP
 *
 * `samplePixel` reads the pixel data offset and bit depth directly from the
 * BMP file header, so the `format` parameter is retained only for API
 * compatibility and is not used by the implementation.
 */

import type { ImageCoords } from "./viewerUtils";

/** A sampled pixel value from the BMP frame buffer. */
export type PixelSample =
  | { kind: "mono"; value: number }
  | { kind: "rgb"; r: number; g: number; b: number };

/**
 * Sample a single pixel from a BMP-encoded frame buffer at image-space coordinates.
 *
 * The buffer must be a complete BMP file as received from the app's streamer:
 * - 8bpp BMP: returns `{ kind: "mono", value: 0–255 }`
 * - 24bpp BMP: returns `{ kind: "rgb", r, g, b }` (channels decoded from BGR)
 *
 * Returns null for out-of-bounds coordinates, a buffer too small to contain a
 * valid BMP header, or an unsupported bit depth.
 *
 * @param bmpBytes  Full BMP file bytes (from WebSocket binary frame).
 * @param x         Image-space X coordinate (integer, 0-based).
 * @param y         Image-space Y coordinate (integer, 0-based).
 * @param imgW      Image width in pixels (from StreamInfo frame).
 * @param imgH      Image height in pixels (from StreamInfo frame).
 * @param _format   Retained for call-site compatibility; not used.
 */
export function samplePixel(
  bmpBytes: Uint8Array,
  x: number,
  y: number,
  imgW: number,
  imgH: number,
  _format: string,
): PixelSample | null {
  if (x < 0 || y < 0 || x >= imgW || y >= imgH) return null;
  // BMP file header is 14 bytes + DIB header 40 bytes = 54 bytes minimum.
  if (bmpBytes.length < 54) return null;

  // Pixel data offset: bytes 10–13, little-endian u32.
  const dataOffset =
    bmpBytes[10] |
    (bmpBytes[11] << 8) |
    (bmpBytes[12] << 16) |
    (bmpBytes[13] << 24);

  // Bits per pixel: bytes 28–29 in BITMAPINFOHEADER, little-endian u16.
  const bitsPerPixel = bmpBytes[28] | (bmpBytes[29] << 8);

  if (bitsPerPixel === 8) {
    // 8bpp grayscale: row stride = imgW rounded up to 4-byte boundary.
    const rowStride = (imgW + 3) & ~3;
    const idx = dataOffset + y * rowStride + x;
    if (idx >= bmpBytes.length) return null;
    return { kind: "mono", value: bmpBytes[idx] };
  }

  if (bitsPerPixel === 24) {
    // 24bpp colour: pixels stored as BGR; row stride = (imgW*3) rounded up to 4 bytes.
    const rowStride = ((imgW * 3) + 3) & ~3;
    const base = dataOffset + y * rowStride + x * 3;
    if (base + 2 >= bmpBytes.length) return null;
    // BMP stores B, G, R — return as R, G, B.
    return {
      kind: "rgb",
      r: bmpBytes[base + 2],
      g: bmpBytes[base + 1],
      b: bmpBytes[base],
    };
  }

  // Unsupported bit depth (e.g. 32bpp or non-standard BMP).
  return null;
}

/**
 * Format a pixel sample as a human-readable string.
 * - Mono: the integer value (0–255 after downscaling)
 * - RGB: "R:N G:N B:N"
 */
export function formatPixelSample(sample: PixelSample): string {
  if (sample.kind === "mono") {
    return String(sample.value);
  }
  return `R:${sample.r} G:${sample.g} B:${sample.b}`;
}

/** Hover info passed from ViewerCanvas to ImageViewer and then to the status bar. */
export interface PixelHoverInfo {
  coords: ImageCoords;
  sample: PixelSample | null;
  formatted: string;
}
