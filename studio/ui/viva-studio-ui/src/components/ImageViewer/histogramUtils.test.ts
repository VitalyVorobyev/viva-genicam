import { describe, it, expect } from "vitest";
import { computeHistogram, renderHistogram } from "./histogramUtils";
import type { HistogramData } from "./histogramUtils";
import { makeGray8Bmp, makeRgb24Bmp } from "../../test/bmp";

// ── computeHistogram tests ─────────────────────────────────────────────────────

describe("computeHistogram", () => {
  it("test_computeHistogram_gray8_all_same", () => {
    // 2×2 uniform image at value 100: bin[100] = 4, rest = 0.
    const bmp = makeGray8Bmp(2, 2, () => 100);
    const result = computeHistogram(bmp);
    expect(result).not.toBeNull();
    expect(result!.kind).toBe("mono");
    const { values } = result as Extract<typeof result, { kind: "mono" }>;
    expect(values[100]).toBe(4);
    expect(values[0]).toBe(0);
    expect(values[255]).toBe(0);
  });

  it("test_computeHistogram_gray8_two_values", () => {
    // 2×2: top row = 0, bottom row = 255. Each 2 pixels.
    const bmp = makeGray8Bmp(2, 2, (_, y) => (y === 0 ? 0 : 255));
    const result = computeHistogram(bmp);
    expect(result).not.toBeNull();
    const { values } = result as Extract<typeof result, { kind: "mono" }>;
    expect(values[0]).toBe(2);
    expect(values[255]).toBe(2);
  });

  it("test_computeHistogram_gray8_row_stride_padding", () => {
    // 3×1 image (stride = 4): 3 valid pixels at value 42.
    const bmp = makeGray8Bmp(3, 1, () => 42);
    const result = computeHistogram(bmp);
    expect(result).not.toBeNull();
    const { values } = result as Extract<typeof result, { kind: "mono" }>;
    // Only 3 pixels counted (padding byte not included).
    expect(values[42]).toBe(3);
  });

  it("test_computeHistogram_rgb24_channels", () => {
    // 1×1 pixel: R=255, G=128, B=64.
    const bmp = makeRgb24Bmp(1, 1, () => [255, 128, 64]);
    const result = computeHistogram(bmp);
    expect(result).not.toBeNull();
    expect(result!.kind).toBe("rgb");
    const { r, g, b } = result as Extract<typeof result, { kind: "rgb" }>;
    expect(r[255]).toBe(1);
    expect(g[128]).toBe(1);
    expect(b[64]).toBe(1);
    // No leakage into other bins.
    expect(r[0]).toBe(0);
    expect(g[0]).toBe(0);
    expect(b[0]).toBe(0);
  });

  it("test_computeHistogram_too_short", () => {
    // Buffer shorter than 54 bytes → null immediately.
    const tiny = new Uint8Array(10);
    expect(computeHistogram(tiny)).toBeNull();
  });

  it("test_computeHistogram_unsupported_bpp", () => {
    // Build a valid 8bpp BMP then overwrite bits-per-pixel to 32.
    const bmp = makeGray8Bmp(1, 1, () => 0);
    bmp[28] = 32; bmp[29] = 0;
    expect(computeHistogram(bmp)).toBeNull();
  });
});

// ── renderHistogram tests ──────────────────────────────────────────────────────

/** Minimal CanvasRenderingContext2D stub for unit tests. */
class FakeCtx {
  ops: string[] = [];

  clearRect(_x: number, _y: number, _w: number, _h: number) {
    this.ops.push("clearRect");
  }

  fillRect(_x: number, _y: number, _w: number, _h: number) {
    this.ops.push("fillRect");
  }

  set fillStyle(_v: string) {
    this.ops.push("fillStyle");
  }
}

describe("renderHistogram", () => {
  it("test_renderHistogram_mono_peak_fills_height", () => {
    // All 4 pixels in bin[0] → peak = 4; that bin fills full height.
    const data: HistogramData = {
      kind: "mono",
      values: new Uint32Array(256), // all zero
    };
    data.values[0] = 4;

    const ctx = new FakeCtx();
    // Should not throw.
    expect(() =>
      renderHistogram(ctx as unknown as CanvasRenderingContext2D, data, 256, 80),
    ).not.toThrow();
    // At least one clearRect and 256 fillRect calls.
    expect(ctx.ops.filter((o) => o === "clearRect").length).toBe(1);
    expect(ctx.ops.filter((o) => o === "fillRect").length).toBe(256);
  });

  it("test_renderHistogram_rgb_draws_three_channels", () => {
    const data: HistogramData = {
      kind: "rgb",
      r: new Uint32Array(256),
      g: new Uint32Array(256),
      b: new Uint32Array(256),
    };
    data.r[200] = 10;
    data.g[100] = 20;
    data.b[50]  = 15;

    const ctx = new FakeCtx();
    expect(() =>
      renderHistogram(ctx as unknown as CanvasRenderingContext2D, data, 256, 80),
    ).not.toThrow();
    // One clearRect; 3 channels × 256 bins each = 768 fillRect calls.
    expect(ctx.ops.filter((o) => o === "clearRect").length).toBe(1);
    expect(ctx.ops.filter((o) => o === "fillRect").length).toBe(768);
  });
});
