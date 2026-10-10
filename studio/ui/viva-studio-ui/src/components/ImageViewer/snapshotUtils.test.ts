import { describe, it, expect, beforeAll } from "vitest";
import { frameToImageData, snapshotImageData, suggestFilename } from "./snapshotUtils";
import { makeGray8Bmp, makeRgb24Bmp } from "../../test/bmp";

// Vitest runs in Node where ImageData is not a global.
// Provide a minimal polyfill so the pure pixel-conversion functions can be tested.
beforeAll(() => {
  if (typeof globalThis.ImageData === "undefined") {
    // @ts-expect-error — polyfill for Node test environment
    globalThis.ImageData = class ImageData {
      readonly data: Uint8ClampedArray;
      readonly width: number;
      readonly height: number;
      constructor(data: Uint8ClampedArray, width: number, height: number) {
        this.data = data;
        this.width = width;
        this.height = height;
      }
    };
  }
});

// ---------------------------------------------------------------------------
// frameToImageData tests
// ---------------------------------------------------------------------------

describe("frameToImageData", () => {
  it("test_frameToImageData_mono8_correct_rgba", () => {
    // 1×1 Mono8 pixel with value 128
    const bytes = new Uint8Array([128]);
    const result = frameToImageData(bytes, 1, 1, "Mono8");
    expect(result).not.toBeNull();
    expect(result!.data[0]).toBe(128); // R
    expect(result!.data[1]).toBe(128); // G
    expect(result!.data[2]).toBe(128); // B
    expect(result!.data[3]).toBe(255); // A
  });

  it("test_frameToImageData_rgb8_correct_rgba", () => {
    // 1×1 RGB8 pixel: R=10, G=20, B=30
    const bytes = new Uint8Array([10, 20, 30]);
    const result = frameToImageData(bytes, 1, 1, "RGB8");
    expect(result).not.toBeNull();
    expect(result!.data[0]).toBe(10);  // R preserved
    expect(result!.data[1]).toBe(20);  // G preserved
    expect(result!.data[2]).toBe(30);  // B preserved
    expect(result!.data[3]).toBe(255); // A
  });

  it("test_frameToImageData_bgr8_swaps_channels", () => {
    // 1×1 BGR8 pixel: B=30, G=20, R=10 → expect RGBA [10, 20, 30, 255]
    const bytes = new Uint8Array([30, 20, 10]);
    const result = frameToImageData(bytes, 1, 1, "BGR8");
    expect(result).not.toBeNull();
    expect(result!.data[0]).toBe(10);  // R (was B position)
    expect(result!.data[1]).toBe(20);  // G unchanged
    expect(result!.data[2]).toBe(30);  // B (was R position)
    expect(result!.data[3]).toBe(255); // A
  });

  it("test_frameToImageData_mono16_uses_high_byte", () => {
    // 1×1 Mono16 LE: low byte = 0x00, high byte = 0xAB → RGBA [0xAB, 0xAB, 0xAB, 255]
    const bytes = new Uint8Array([0x00, 0xab]);
    const result = frameToImageData(bytes, 1, 1, "Mono16");
    expect(result).not.toBeNull();
    expect(result!.data[0]).toBe(0xab); // R = high byte
    expect(result!.data[1]).toBe(0xab); // G = high byte
    expect(result!.data[2]).toBe(0xab); // B = high byte
    expect(result!.data[3]).toBe(255);  // A
  });

  it("test_frameToImageData_unknown_format_returns_null", () => {
    const bytes = new Uint8Array([100, 150, 200]);
    const result = frameToImageData(bytes, 1, 1, "YUV422");
    expect(result).toBeNull();
  });

  it("test_frameToImageData_truncated_buffer_returns_null", () => {
    // RGB8 requires 3 bytes for 1×1; give only 2
    const bytes = new Uint8Array([10, 20]);
    const result = frameToImageData(bytes, 1, 1, "RGB8");
    expect(result).toBeNull();
  });

  it("test_frameToImageData_bayer_treated_as_mono", () => {
    // BayerRG8 1×1 pixel with value 200 → grayscale RGBA
    const bytes = new Uint8Array([200]);
    const result = frameToImageData(bytes, 1, 1, "BayerRG8");
    expect(result).not.toBeNull();
    expect(result!.data[0]).toBe(200); // R
    expect(result!.data[1]).toBe(200); // G
    expect(result!.data[2]).toBe(200); // B
    expect(result!.data[3]).toBe(255); // A
  });
});

// ---------------------------------------------------------------------------
// suggestFilename tests
// ---------------------------------------------------------------------------

// The viewer's WebSocket delivers BMP files (`viva_streamer::bmp`), and a
// snapshot is made from the last one received.
describe("snapshotImageData", () => {
  it("decodes an 8bpp grayscale BMP frame", () => {
    const bmp = makeGray8Bmp(2, 2, (x, y) => [10, 20, 30, 40][y * 2 + x]!);
    const image = snapshotImageData(bmp, { width: 2, height: 2, pixelFormat: "Mono8" });
    expect(image?.width).toBe(2);
    expect(image?.height).toBe(2);
    expect(Array.from(image!.data)).toEqual([
      10, 10, 10, 255, 20, 20, 20, 255,
      30, 30, 30, 255, 40, 40, 40, 255,
    ]);
  });

  it("decodes a 24bpp BMP frame from a Bayer camera", () => {
    const bmp = makeRgb24Bmp(2, 1, (x) => (x === 0 ? [255, 0, 0] : [0, 128, 255]));
    const image = snapshotImageData(bmp, { width: 2, height: 1, pixelFormat: "BayerRG8" });
    expect(Array.from(image!.data)).toEqual([255, 0, 0, 255, 0, 128, 255, 255]);
  });

  it("reads a bottom-up BMP top row first", () => {
    const bmp = makeGray8Bmp(1, 2, (_x, y) => (y === 0 ? 1 : 2), { topDown: false });
    const image = snapshotImageData(bmp, { width: 1, height: 2, pixelFormat: "Mono8" });
    expect(Array.from(image!.data)).toEqual([1, 1, 1, 255, 2, 2, 2, 255]);
  });

  it("refuses a buffer that is not a BMP", () => {
    expect(snapshotImageData(new Uint8Array(64), { width: 4, height: 4, pixelFormat: "Mono8" })).toBeNull();
  });
});

describe("suggestFilename", () => {
  it("test_suggestFilename_contains_dimensions", () => {
    const name = suggestFilename("Mono8", 640, 480);
    expect(name).toContain("640x480");
  });

  it("test_suggestFilename_no_colons", () => {
    const name = suggestFilename("RGB8", 1920, 1080);
    expect(name).not.toContain(":");
  });

  it("test_suggestFilename_format_in_name", () => {
    const name = suggestFilename("BayerRG8", 320, 240);
    expect(name).toContain("BayerRG8");
  });
});
