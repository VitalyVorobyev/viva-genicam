/**
 * Decode a BMP file — what `viva_streamer::bmp` sends over the viewer's
 * WebSocket — into RGBA pixels.
 *
 * Handles what the streamer writes and what a BMP reader must expect:
 * - 8bpp through its colour palette (the streamer's is identity grayscale);
 * - 24bpp, stored B, G, R;
 * - rows padded to 4 bytes;
 * - top-down (negative height) and bottom-up row order.
 *
 * Returns null for anything else — a buffer without the `BM` signature, a
 * compressed or other-depth image, or one shorter than its header claims.
 */
export function decodeBmp(
  bytes: Uint8Array,
): { width: number; height: number; data: Uint8ClampedArray<ArrayBuffer> } | null {
  if (bytes.length < 54 || bytes[0] !== 0x42 || bytes[1] !== 0x4d) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const dataOffset = view.getUint32(10, true);
  const headerSize = view.getUint32(14, true);
  const width = view.getInt32(18, true);
  const signedHeight = view.getInt32(22, true);
  const bitsPerPixel = view.getUint16(28, true);
  const compression = view.getUint32(30, true);
  if (width <= 0 || signedHeight === 0 || compression !== 0) return null;
  if (bitsPerPixel !== 8 && bitsPerPixel !== 24) return null;

  const height = Math.abs(signedHeight);
  const topDown = signedHeight < 0;
  const rowStride = ((width * bitsPerPixel) / 8 + 3) & ~3;
  if (dataOffset + rowStride * height > bytes.length) return null;

  const paletteOffset = 14 + headerSize;
  if (bitsPerPixel === 8 && paletteOffset + 256 * 4 > dataOffset) return null;

  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    const row = dataOffset + (topDown ? y : height - 1 - y) * rowStride;
    for (let x = 0; x < width; x++) {
      const out = (y * width + x) * 4;
      // Both layouts store a pixel as B, G, R: in the palette entry for 8bpp,
      // in place for 24bpp.
      const src = bitsPerPixel === 8 ? paletteOffset + bytes[row + x]! * 4 : row + x * 3;
      data[out] = bytes[src + 2]!;
      data[out + 1] = bytes[src + 1]!;
      data[out + 2] = bytes[src]!;
      data[out + 3] = 255;
    }
  }
  return { width, height, data };
}

/**
 * The image a snapshot saves: the frame the viewer last received, as the
 * viewer shows it. The streamer has already reduced the camera's pixel format
 * to 8-bit gray or RGB, so this is not a raw-depth capture.
 */
export function snapshotImageData(frame: Uint8Array): ImageData | null {
  const decoded = decodeBmp(frame);
  if (!decoded) return null;
  return new ImageData(decoded.data, decoded.width, decoded.height);
}

/**
 * Suggest a safe filename for a snapshot, e.g.:
 *   snapshot_640x480_Mono8_20260308T153042Z.png
 *
 * Uses ISO 8601 timestamp with `:` and `.` stripped to keep it filesystem-safe.
 */
export function suggestFilename(
  pixelFormat: string,
  width: number,
  height: number,
): string {
  const ts = new Date()
    .toISOString()
    .replace(/:/g, "")
    .replace(/\./g, "-")
    .replace(/-\d+Z$/, "Z");
  return `snapshot_${width}x${height}_${pixelFormat}_${ts}.png`;
}

/**
 * Encode ImageData to a PNG Blob.
 *
 * Tries OffscreenCanvas.convertToBlob first; falls back to
 * document.createElement("canvas") + toBlob for environments where
 * OffscreenCanvas is unavailable.
 *
 * Returns null if encoding fails.
 */
export async function encodeImageDataToPng(
  imageData: ImageData,
): Promise<Blob | null> {
  const { width, height } = imageData;

  // Try OffscreenCanvas first
  if (typeof OffscreenCanvas !== "undefined") {
    try {
      const oc = new OffscreenCanvas(width, height);
      const ctx = oc.getContext("2d");
      if (ctx) {
        ctx.putImageData(imageData, 0, 0);
        return await oc.convertToBlob({ type: "image/png" });
      }
    } catch {
      // Fall through to the canvas fallback
    }
  }

  // Fallback: HTMLCanvasElement
  return new Promise((resolve) => {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      resolve(null);
      return;
    }
    ctx.putImageData(imageData, 0, 0);
    canvas.toBlob((blob) => resolve(blob), "image/png");
  });
}
