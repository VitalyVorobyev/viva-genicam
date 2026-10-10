/**
 * BMP builders for tests, matching what `viva_streamer::bmp` sends over the
 * WebSocket: an 8bpp image with a 256-entry identity grayscale palette, or a
 * 24bpp BGR image, both with rows padded to 4 bytes. Rows are top-down
 * (negative height) as the streamer writes them unless `topDown` is false.
 */

export function makeGray8Bmp(
  width: number,
  height: number,
  pixelFn: (x: number, y: number) => number,
  { topDown = true }: { topDown?: boolean } = {},
): Uint8Array {
  const rowStride = (width + 3) & ~3;
  const paletteSize = 256 * 4; // BGRA quads
  const dataOffset = 14 + 40 + paletteSize;
  const imageSize = rowStride * height;
  const fileSize = dataOffset + imageSize;

  const buf = new Uint8Array(fileSize);

  // BITMAPFILEHEADER (14 bytes)
  buf[0] = 0x42; buf[1] = 0x4d; // "BM"
  writeU32LE(buf, 2, fileSize);
  writeU32LE(buf, 10, dataOffset);

  // BITMAPINFOHEADER (40 bytes, starting at offset 14)
  writeU32LE(buf, 14, 40);                           // header size
  writeU32LE(buf, 18, width);                        // width
  writeI32LE(buf, 22, topDown ? -height : height);   // negative = top-down
  writeU16LE(buf, 26, 1);                            // planes
  writeU16LE(buf, 28, 8);                            // bits per pixel
  writeU32LE(buf, 30, 0);                            // compression (BI_RGB)
  writeU32LE(buf, 34, imageSize);                    // image size
  writeU32LE(buf, 46, 256);                          // colors used

  // Palette: identity grayscale (B=G=R=i, A=0)
  for (let i = 0; i < 256; i++) {
    const base = 54 + i * 4;
    buf[base] = i; buf[base + 1] = i; buf[base + 2] = i; buf[base + 3] = 0;
  }

  for (let y = 0; y < height; y++) {
    const row = topDown ? y : height - 1 - y;
    for (let x = 0; x < width; x++) {
      buf[dataOffset + row * rowStride + x] = pixelFn(x, y);
    }
  }

  return buf;
}

/** `pixelFn` returns [R, G, B]; the BMP stores B, G, R. */
export function makeRgb24Bmp(
  width: number,
  height: number,
  pixelFn: (x: number, y: number) => [number, number, number],
  { topDown = true }: { topDown?: boolean } = {},
): Uint8Array {
  const rowStride = ((width * 3) + 3) & ~3;
  const dataOffset = 14 + 40; // no palette
  const imageSize = rowStride * height;
  const fileSize = dataOffset + imageSize;

  const buf = new Uint8Array(fileSize);

  // BITMAPFILEHEADER
  buf[0] = 0x42; buf[1] = 0x4d; // "BM"
  writeU32LE(buf, 2, fileSize);
  writeU32LE(buf, 10, dataOffset);

  // BITMAPINFOHEADER
  writeU32LE(buf, 14, 40);
  writeU32LE(buf, 18, width);
  writeI32LE(buf, 22, topDown ? -height : height);
  writeU16LE(buf, 26, 1);
  writeU16LE(buf, 28, 24); // bits per pixel
  writeU32LE(buf, 34, imageSize);

  for (let y = 0; y < height; y++) {
    const row = topDown ? y : height - 1 - y;
    for (let x = 0; x < width; x++) {
      const [r, g, b] = pixelFn(x, y);
      const base = dataOffset + row * rowStride + x * 3;
      buf[base] = b; buf[base + 1] = g; buf[base + 2] = r;
    }
  }

  return buf;
}

function writeU32LE(buf: Uint8Array, offset: number, value: number): void {
  buf[offset]     = value & 0xff;
  buf[offset + 1] = (value >> 8) & 0xff;
  buf[offset + 2] = (value >> 16) & 0xff;
  buf[offset + 3] = (value >> 24) & 0xff;
}

function writeI32LE(buf: Uint8Array, offset: number, value: number): void {
  new DataView(buf.buffer, buf.byteOffset + offset, 4).setInt32(0, value, true);
}

function writeU16LE(buf: Uint8Array, offset: number, value: number): void {
  buf[offset]     = value & 0xff;
  buf[offset + 1] = (value >> 8) & 0xff;
}
