/** Returns the expected byte count for a given pixel format and dimensions. */
function expectedByteCount(
  width: number,
  height: number,
  pixelFormat: string,
): number {
  if (pixelFormat === "Mono16") return width * height * 2;
  if (pixelFormat === "RGB8" || pixelFormat === "BGR8") return width * height * 3;
  // Mono8 and all Bayer*8 formats: 1 byte per pixel
  return width * height;
}

/**
 * Convert raw frame bytes to RGBA ImageData for PNG encoding.
 *
 * Supported formats:
 * - Mono8 and all Bayer*8: each byte v → [v, v, v, 255]
 * - RGB8: [r, g, b] → [r, g, b, 255]
 * - BGR8: [b, g, r] → [r, g, b, 255]
 * - Mono16: 2 bytes LE → high byte used as 8-bit approximation → [hb, hb, hb, 255]
 *   // best-effort until ST-02 delivers 16-bit depth
 * - Unknown: returns null
 *
 * Returns null if bytes.length < expectedByteCount or format is unsupported.
 */
export function frameToImageData(
  bytes: Uint8Array,
  width: number,
  height: number,
  pixelFormat: string,
): ImageData | null {
  if (bytes.length < expectedByteCount(width, height, pixelFormat)) {
    return null;
  }

  const pixels = width * height;
  const rgba = new Uint8ClampedArray(pixels * 4);

  if (pixelFormat === "Mono8" || pixelFormat.startsWith("Bayer")) {
    for (let i = 0; i < pixels; i++) {
      const v = bytes[i]!;
      rgba[i * 4] = v;
      rgba[i * 4 + 1] = v;
      rgba[i * 4 + 2] = v;
      rgba[i * 4 + 3] = 255;
    }
    return new ImageData(rgba, width, height);
  }

  if (pixelFormat === "RGB8") {
    for (let i = 0; i < pixels; i++) {
      rgba[i * 4] = bytes[i * 3]!;
      rgba[i * 4 + 1] = bytes[i * 3 + 1]!;
      rgba[i * 4 + 2] = bytes[i * 3 + 2]!;
      rgba[i * 4 + 3] = 255;
    }
    return new ImageData(rgba, width, height);
  }

  if (pixelFormat === "BGR8") {
    for (let i = 0; i < pixels; i++) {
      rgba[i * 4] = bytes[i * 3 + 2]!;
      rgba[i * 4 + 1] = bytes[i * 3 + 1]!;
      rgba[i * 4 + 2] = bytes[i * 3]!;
      rgba[i * 4 + 3] = 255;
    }
    return new ImageData(rgba, width, height);
  }

  if (pixelFormat === "Mono16") {
    // best-effort until ST-02 delivers 16-bit depth
    for (let i = 0; i < pixels; i++) {
      const hb = bytes[i * 2 + 1]!;
      rgba[i * 4] = hb;
      rgba[i * 4 + 1] = hb;
      rgba[i * 4 + 2] = hb;
      rgba[i * 4 + 3] = 255;
    }
    return new ImageData(rgba, width, height);
  }

  return null;
}

/**
 * The image a snapshot saves, from the frame the viewer last received.
 */
export function snapshotImageData(
  frame: Uint8Array,
  meta: { width: number; height: number; pixelFormat: string },
): ImageData | null {
  return frameToImageData(frame, meta.width, meta.height, meta.pixelFormat);
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
