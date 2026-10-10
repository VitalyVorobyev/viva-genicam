import { describe, it, expect, beforeAll } from "vitest";
import { snapshotImageData, suggestFilename } from "./snapshotUtils";
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

// The viewer's WebSocket delivers BMP files (`viva_streamer::bmp`), and a
// snapshot is made from the last one received.
describe("snapshotImageData", () => {
  it("decodes an 8bpp grayscale BMP frame", () => {
    const bmp = makeGray8Bmp(2, 2, (x, y) => [10, 20, 30, 40][y * 2 + x]!);
    const image = snapshotImageData(bmp);
    expect(image?.width).toBe(2);
    expect(image?.height).toBe(2);
    expect(Array.from(image!.data)).toEqual([
      10, 10, 10, 255, 20, 20, 20, 255,
      30, 30, 30, 255, 40, 40, 40, 255,
    ]);
  });

  it("decodes a 24bpp BMP frame from a Bayer camera", () => {
    const bmp = makeRgb24Bmp(2, 1, (x) => (x === 0 ? [255, 0, 0] : [0, 128, 255]));
    const image = snapshotImageData(bmp);
    expect(Array.from(image!.data)).toEqual([255, 0, 0, 255, 0, 128, 255, 255]);
  });

  it("reads a bottom-up BMP top row first", () => {
    const bmp = makeGray8Bmp(1, 2, (_x, y) => (y === 0 ? 1 : 2), { topDown: false });
    const image = snapshotImageData(bmp);
    expect(Array.from(image!.data)).toEqual([1, 1, 1, 255, 2, 2, 2, 255]);
  });

  it("refuses a buffer that is not a BMP", () => {
    expect(snapshotImageData(new Uint8Array(64))).toBeNull();
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
