/** Canonical samples decoded by Rust's PFNC implementation, never PFNC in JS.
 * A capture includes the matching preview and native-depth PNG so freezing,
 * inspection and saving cannot accidentally combine different camera frames.
 */
import type { PixelSample } from "./pixelInspectorUtils";
import type { HistogramData } from "./histogramUtils";
import { bresenhamLine, type ProfileData } from "./lineProfileUtils";

export interface NativeFrame {
  width: number;
  height: number;
  pixelFormat: string;
  bits: number;
  channels: 1 | 3;
  mosaic: boolean;
  bmp: Uint8Array<ArrayBuffer>;
  png: Uint8Array<ArrayBuffer>;
  samples: Uint8Array<ArrayBuffer> | Uint16Array<ArrayBuffer>;
}

const MAX_PIXELS = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES = 256 * 1024 * 1024;
const MAX_ERROR_BYTES = 4096;
const integer = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value > 0;

export function parseNativeFrame(buffer: ArrayBuffer): NativeFrame {
  if (buffer.byteLength > MAX_RESPONSE_BYTES) throw new Error("Native capture exceeds 256 MiB");
  const bytes = new Uint8Array(buffer);
  const view = new DataView(buffer);
  if (bytes.length < 8 || String.fromCharCode(...bytes.subarray(0, 4)) !== "VRAW") throw new Error("Invalid native capture signature");
  const headerLength = view.getUint32(4, true);
  if (headerLength < 2 || headerLength > 4096 || 8 + headerLength > bytes.length) throw new Error("Invalid native capture header");
  const h: Record<string, unknown> = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(8, 8 + headerLength)));
  if (h.version !== 1 || !integer(h.width) || !integer(h.height) || h.width * h.height > MAX_PIXELS
      || !integer(h.bits) || h.bits < 8 || h.bits > 16 || (h.channels !== 1 && h.channels !== 3)
      || typeof h.mosaic !== "boolean" || typeof h.pixel_format !== "string" || h.pixel_format.length > 128
      || !integer(h.bmp_bytes) || !integer(h.sample_bytes) || !integer(h.png_bytes)) throw new Error("Unsupported native capture metadata");
  const expected = h.width * h.height * h.channels * (h.bits > 8 ? 2 : 1);
  if (h.sample_bytes !== expected || 8 + headerLength + h.bmp_bytes + expected + h.png_bytes !== bytes.length) throw new Error("Native capture length mismatch");
  let at = 8 + headerLength;
  const bmp = bytes.slice(at, at += h.bmp_bytes);
  const bmpView = new DataView(bmp.buffer);
  if (bmp.length < 54 || bmp[0] !== 0x42 || bmp[1] !== 0x4d || bmpView.getInt32(18, true) !== h.width || Math.abs(bmpView.getInt32(22, true)) !== h.height) throw new Error("Native capture preview geometry mismatch");
  const samples = h.bits > 8 ? new Uint16Array(h.width * h.height * h.channels) : bytes.slice(at, at + expected);
  if (samples instanceof Uint16Array) {
    for (let i = 0; i < samples.length; i++) samples[i] = view.getUint16(at + i * 2, true);
  }
  const maximum = 2 ** h.bits - 1;
  if (samples.some(value => value > maximum)) throw new Error("Native sample exceeds its bit depth");
  at += expected;
  const png = bytes.slice(at);
  if (png.length < 8 || png[0] !== 0x89 || png[1] !== 0x50 || png[2] !== 0x4e || png[3] !== 0x47) throw new Error("Invalid native PNG");
  return { width: h.width, height: h.height, pixelFormat: h.pixel_format, bits: h.bits, channels: h.channels, mosaic: h.mosaic, bmp, png, samples };
}

export async function captureNativeFrame(wsUrl: string, signal?: AbortSignal): Promise<NativeFrame> {
  const url = new URL(wsUrl);
  if (!["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)) throw new Error("Native capture requires a local streamer");
  url.protocol = url.protocol === "wss:" ? "https:" : "http:";
  url.pathname += "/native";
  const response = await fetch(url, { signal, cache: "no-store" });
  if (!response.ok) {
    let detail: string;
    try {
      detail = new TextDecoder().decode(await readBoundedResponse(response, MAX_ERROR_BYTES));
    } catch {
      detail = "Response exceeded the error-message limit or could not be read";
    }
    throw new Error(`Native capture failed (HTTP ${response.status}): ${detail}`);
  }
  const bytes = await readBoundedResponse(response, MAX_RESPONSE_BYTES);
  return parseNativeFrame(bytes.buffer);
}

/** Enforce the actual decoded stream length, even without Content-Length or
 * when a stale local server lies about it. At most limit bytes are retained;
 * crossing the limit cancels the body rather than buffering its remainder.
 */
export async function readBoundedResponse(response: Response, limit: number): Promise<Uint8Array<ArrayBuffer>> {
  const declared = response.headers.get("content-length");
  if (declared !== null && (!/^\d+$/.test(declared) || !Number.isSafeInteger(Number(declared)) || Number(declared) > limit)) {
    await response.body?.cancel().catch(() => {});
    throw new Error("Native capture response exceeds the byte limit");
  }
  const reader = response.body?.getReader();
  if (!reader) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      if (value.byteLength > limit - length) throw new Error("Native capture response exceeds the byte limit");
      length += value.byteLength;
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

export function sampleNativePixel(frame: NativeFrame, x: number, y: number): PixelSample | null {
  if (!Number.isInteger(x) || !Number.isInteger(y) || x < 0 || y < 0 || x >= frame.width || y >= frame.height) return null;
  const at = (y * frame.width + x) * frame.channels;
  return frame.channels === 1 ? { kind: "mono", value: frame.samples[at] } : { kind: "rgb", r: frame.samples[at], g: frame.samples[at + 1], b: frame.samples[at + 2] };
}

export function nativeHistogram(frame: NativeFrame): HistogramData {
  const channels = Array.from({ length: frame.channels }, () => new Uint32Array(2 ** frame.bits));
  for (let i = 0; i < frame.samples.length; i++) channels[i % frame.channels][frame.samples[i]]++;
  return frame.channels === 1 ? { kind: "mono", values: channels[0] } : { kind: "rgb", r: channels[0], g: channels[1], b: channels[2] };
}

export function nativeLineProfile(frame: NativeFrame, x0: number, y0: number, x1: number, y1: number): ProfileData {
  const clamp = (v: number, end: number) => Math.max(0, Math.min(Math.round(v), end - 1));
  const points = bresenhamLine(clamp(x0, frame.width), clamp(y0, frame.height), clamp(x1, frame.width), clamp(y1, frame.height));
  const channels = Array.from({ length: frame.channels }, () => new Float32Array(points.length));
  points.forEach((p, i) => {
    const at = (p.y * frame.width + p.x) * frame.channels;
    channels.forEach((channel, c) => { channel[i] = frame.samples[at + c]; });
  });
  return frame.channels === 1 ? { kind: "mono", values: channels[0], length: points.length } : { kind: "rgb", r: channels[0], g: channels[1], b: channels[2], length: points.length };
}
