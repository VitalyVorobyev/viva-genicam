import { describe, it, expect } from "vitest";
import { bresenhamLine, sampleLineProfile, renderLineProfile } from "./lineProfileUtils";
import type { ProfileData } from "./lineProfileUtils";
import { makeGray8Bmp, makeRgb24Bmp } from "../../test/bmp";

// ── bresenhamLine tests ────────────────────────────────────────────────────────

describe("bresenhamLine", () => {
  it("test_bresenhamLine_single_point", () => {
    // start === end → exactly one pixel
    const result = bresenhamLine(5, 7, 5, 7);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ x: 5, y: 7 });
  });

  it("test_bresenhamLine_horizontal_right", () => {
    // 4-pixel horizontal run: (0,0)→(3,0)
    const result = bresenhamLine(0, 0, 3, 0);
    expect(result).toHaveLength(4);
    expect(result.every((p) => p.y === 0)).toBe(true);
    expect(result.map((p) => p.x)).toEqual([0, 1, 2, 3]);
  });

  it("test_bresenhamLine_vertical_down", () => {
    // 3-pixel vertical run: (2,0)→(2,2)
    const result = bresenhamLine(2, 0, 2, 2);
    expect(result).toHaveLength(3);
    expect(result.every((p) => p.x === 2)).toBe(true);
    expect(result.map((p) => p.y)).toEqual([0, 1, 2]);
  });

  it("test_bresenhamLine_diagonal_45", () => {
    // 45° diagonal: (0,0)→(3,3) → 4 pixels
    const result = bresenhamLine(0, 0, 3, 3);
    expect(result).toHaveLength(4);
    expect(result[0]).toEqual({ x: 0, y: 0 });
    expect(result[3]).toEqual({ x: 3, y: 3 });
    // Each step advances both x and y by 1
    for (let i = 1; i < result.length; i++) {
      expect(result[i]!.x - result[i - 1]!.x).toBe(1);
      expect(result[i]!.y - result[i - 1]!.y).toBe(1);
    }
  });

  it("test_bresenhamLine_reversed", () => {
    // Reversed direction: (3,0)→(0,0) — same pixels in reverse order
    const fwd = bresenhamLine(0, 0, 3, 0);
    const rev = bresenhamLine(3, 0, 0, 0);
    expect(rev).toHaveLength(fwd.length);
    expect(rev.map((p) => p.x)).toEqual([3, 2, 1, 0]);
  });
});

// ── sampleLineProfile tests ────────────────────────────────────────────────────

describe("sampleLineProfile", () => {
  it("test_sampleLineProfile_gray8_horizontal", () => {
    // 4×1 image: pixels 10, 20, 30, 40 along row 0.
    const bmp = makeGray8Bmp(4, 1, (x) => (x + 1) * 10);
    const result = sampleLineProfile(bmp, 0, 0, 3, 0);
    expect(result).not.toBeNull();
    expect(result!.kind).toBe("mono");
    const { values, length } = result as Extract<typeof result, { kind: "mono" }>;
    expect(length).toBe(4);
    expect(Array.from(values)).toEqual([10, 20, 30, 40]);
  });

  it("test_sampleLineProfile_too_short_returns_null", () => {
    const tiny = new Uint8Array(10);
    expect(sampleLineProfile(tiny, 0, 0, 1, 0)).toBeNull();
  });

  it("test_sampleLineProfile_unsupported_bpp_returns_null", () => {
    const bmp = makeGray8Bmp(2, 2, () => 0);
    bmp[28] = 32; bmp[29] = 0; // overwrite bpp to 32
    expect(sampleLineProfile(bmp, 0, 0, 1, 0)).toBeNull();
  });

  it("test_sampleLineProfile_rgb24_single_pixel", () => {
    // 1×1 24bpp BMP: R=100, G=150, B=200
    const bmp = makeRgb24Bmp(1, 1, () => [100, 150, 200]);
    const result = sampleLineProfile(bmp, 0, 0, 0, 0);
    expect(result).not.toBeNull();
    expect(result!.kind).toBe("rgb");
    const { r, g, b } = result as Extract<typeof result, { kind: "rgb" }>;
    expect(r[0]).toBe(100);
    expect(g[0]).toBe(150);
    expect(b[0]).toBe(200);
  });

  it("test_sampleLineProfile_out_of_bounds_clamped", () => {
    // Coordinates beyond image are clamped; should not return null.
    const bmp = makeGray8Bmp(4, 4, () => 128);
    const result = sampleLineProfile(bmp, -5, 0, 100, 0);
    expect(result).not.toBeNull();
    expect(result!.kind).toBe("mono");
  });
});

// ── renderLineProfile tests ────────────────────────────────────────────────────

class FakeCtx {
  strokeCalls = 0;
  beginPathCalls = 0;
  ops: string[] = [];

  clearRect() { this.ops.push("clearRect"); }
  beginPath() { this.beginPathCalls++; this.ops.push("beginPath"); }
  moveTo() { this.ops.push("moveTo"); }
  lineTo() { this.ops.push("lineTo"); }
  stroke() { this.strokeCalls++; this.ops.push("stroke"); }
  set strokeStyle(_: string) {}
  set lineWidth(_: number) {}
}

describe("renderLineProfile", () => {
  it("test_renderLineProfile_mono_calls_stroke", () => {
    const data: ProfileData = {
      kind: "mono",
      values: new Float32Array([0, 128, 255]),
      length: 3,
    };
    const ctx = new FakeCtx();
    expect(() =>
      renderLineProfile(ctx as unknown as CanvasRenderingContext2D, data, 256, 80),
    ).not.toThrow();
    expect(ctx.strokeCalls).toBe(1);
    expect(ctx.beginPathCalls).toBe(1);
  });

  it("test_renderLineProfile_rgb_calls_stroke_three_times", () => {
    const data: ProfileData = {
      kind: "rgb",
      r: new Float32Array([255, 0]),
      g: new Float32Array([0, 200]),
      b: new Float32Array([100, 50]),
      length: 2,
    };
    const ctx = new FakeCtx();
    expect(() =>
      renderLineProfile(ctx as unknown as CanvasRenderingContext2D, data, 256, 80),
    ).not.toThrow();
    expect(ctx.strokeCalls).toBe(3);
    expect(ctx.beginPathCalls).toBe(3);
  });

  it("test_renderLineProfile_single_point_no_stroke", () => {
    // length < 2 → nothing drawn after clearRect
    const data: ProfileData = {
      kind: "mono",
      values: new Float32Array([128]),
      length: 1,
    };
    const ctx = new FakeCtx();
    renderLineProfile(ctx as unknown as CanvasRenderingContext2D, data, 256, 80);
    expect(ctx.strokeCalls).toBe(0);
  });
});
