import { afterEach, describe, expect, it, vi } from "vitest";
import { makeGray8Bmp, makeRgb24Bmp } from "../../test/bmp";
import { captureNativeFrame, nativeHistogram, nativeLineProfile, parseNativeFrame, readBoundedResponse, sampleNativePixel } from "./nativeFrame";
import { formatHistogramStats, histogramStats } from "./histogramUtils";

function packet(options: Record<string, unknown> = {}, values = [0xabc, 0xfff]): ArrayBuffer {
  const rgb = options.channels === 3;
  const bmp = rgb ? makeRgb24Bmp(2, 1, () => [10, 20, 30]) : makeGray8Bmp(2, 1, x => x === 0 ? 0xab : 0xff);
  const png = Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const header = new TextEncoder().encode(JSON.stringify({ version: 1, width: 2, height: 1, pixel_format: "Mono12", bits: 12, channels: 1, mosaic: false,
    bmp_bytes: bmp.length, sample_bytes: values.length * 2, png_bytes: png.length, ...options }));
  const result = new Uint8Array(8 + header.length + bmp.length + values.length * 2 + png.length);
  result.set(new TextEncoder().encode("VRAW"));
  const view = new DataView(result.buffer);
  view.setUint32(4, header.length, true);
  result.set(header, 8);result.set(bmp, 8 + header.length);
  values.forEach((value, i) => view.setUint16(8 + header.length + bmp.length + i * 2, value, true));
  result.set(png, result.length - png.length);
  return result.buffer;
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("native frame capture", () => {
  it("preserves 12-bit values in pixels, histogram and line profile rather than preview levels", () => {
    const frame = parseNativeFrame(packet());
    expect(sampleNativePixel(frame, 0, 0)).toEqual({ kind: "mono", value: 0xabc });
    expect(sampleNativePixel(frame, 1, 0)).toEqual({ kind: "mono", value: 4095 });
    expect(sampleNativePixel(frame, 2, 0)).toBeNull();
    const histogram = nativeHistogram(frame);
    expect(histogramStats(histogram)).toEqual({ min: 2748, max: 4095, mean: 3421.5, saturated: 0.5 });
    expect(formatHistogramStats(histogramStats(histogram)!, 4095)).toContain("50.0% at 4095");
    const profile = nativeLineProfile(frame, -10, 0, 10, 0);
    expect(profile.kind === "mono" && Array.from(profile.values)).toEqual([2748, 4095]);
    expect(frame.png).toEqual(Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10]));
  });

  it("samples canonical RGB channels and identifies raw Bayer mosaics", () => {
    const rgb = parseNativeFrame(packet({ channels: 3, pixel_format: "RGB16", bits: 16 }, [1, 256, 65535, 4, 5, 6]));
    expect(sampleNativePixel(rgb, 0, 0)).toEqual({ kind: "rgb", r: 1, g: 256, b: 65535 });
    const mosaic = parseNativeFrame(packet({ mosaic: true, pixel_format: "BayerRG12" }));
    expect(mosaic.mosaic).toBe(true);
    expect(sampleNativePixel(mosaic, 0, 0)).toEqual({ kind: "mono", value: 2748 });
  });

  it("rejects bad versions, oversized geometry, short bodies and values wider than their depth", () => {
    for (const options of [{ version: 2 }, { width: 0 }, { width: 2 ** 30 }, { sample_bytes: 2 }, { bits: 7 }, { channels: 2 }]) {
      expect(() => parseNativeFrame(packet(options))).toThrow();
    }
    expect(() => parseNativeFrame(packet().slice(0, -1))).toThrow(/length/);
    expect(() => parseNativeFrame(packet({}, [65535, 4095]))).toThrow(/bit depth/);
    const badMagic = packet();new Uint8Array(badMagic)[0] = 0;
    expect(() => parseNativeFrame(badMagic)).toThrow(/signature/);
  });

  it("requests a single on-demand capture only from the local streamer", async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(packet()));
    vi.stubGlobal("fetch", fetch);
    await expect(captureNativeFrame("ws://example.com/ws")).rejects.toThrow(/local streamer/);
    expect(fetch).not.toHaveBeenCalled();
    const signal = new AbortController().signal;
    await captureNativeFrame("ws://127.0.0.1:3001/ws", signal);
    expect(String(fetch.mock.calls[0][0])).toBe("http://127.0.0.1:3001/ws/native");
    expect(fetch.mock.calls[0][1]).toEqual({ signal, cache: "no-store" });
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("bounds actual streamed bytes with missing or misleading content lengths", async () => {
    for (const headers of [new Headers(), new Headers({ "Content-Length": "1" })]) {
      const cancelled = vi.fn();
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new Uint8Array(8));
          controller.enqueue(new Uint8Array(9));
        },
        cancel: cancelled,
      });
      await expect(readBoundedResponse(new Response(stream, { headers }), 16)).rejects.toThrow(/byte limit/);
      expect(cancelled).toHaveBeenCalledOnce();
    }
    const complete = new Response(new ReadableStream<Uint8Array>({
      start(controller) { controller.enqueue(Uint8Array.of(1, 2)); controller.enqueue(Uint8Array.of(3, 4)); controller.close(); },
    }));
    expect(await readBoundedResponse(complete, 4)).toEqual(Uint8Array.of(1, 2, 3, 4));
  });

  it("rejects oversized declared bodies before reading and caps error messages", async () => {
    const cancelled = vi.fn();
    const stream = new ReadableStream<Uint8Array>({ cancel: cancelled });
    await expect(readBoundedResponse(new Response(stream, { headers: { "Content-Length": "17" } }), 16)).rejects.toThrow(/byte limit/);
    expect(cancelled).toHaveBeenCalledOnce();
    const fetch = vi.fn().mockResolvedValue(new Response("x".repeat(8192), { status: 422 }));
    vi.stubGlobal("fetch", fetch);
    await expect(captureNativeFrame("ws://127.0.0.1:3001/ws")).rejects.toThrow("HTTP 422): Response exceeded the error-message limit");
  });
});
