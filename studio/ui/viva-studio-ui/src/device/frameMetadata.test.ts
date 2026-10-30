import { describe, expect, it } from "vitest";
import { parseFrameMetadata, parsePreviewFrame } from "./frameMetadata";

export const metadata = {
  block_id: "18446744073709551615", ts_dev: "18446744073709551614", ts_host_ns: null,
  complete: true, chunks: [{ id: "42", bytes: 8 }],
  features: [{ name: "ChunkTimestamp", value: "18446744073709551614", error: null }],
  chunk_error: null, truncated: false,
};

export function previewPacket(value: unknown = metadata): ArrayBuffer {
  const encoded = new TextEncoder().encode(JSON.stringify(value));
  const packet = new Uint8Array(8 + encoded.length + 2);
  packet.set([86, 70, 82, 77]);
  new DataView(packet.buffer).setUint32(4, encoded.length, true);
  packet.set(encoded, 8); packet.set([66, 77], 8 + encoded.length);
  return packet.buffer;
}

describe("frame metadata envelopes", () => {
  it("keeps exact 64-bit values with their preview without copying its BMP", () => {
    const buffer = previewPacket();
    const result = parsePreviewFrame(buffer);
    expect(result.metadata).toEqual(metadata);
    expect(result.bmp).toEqual(new Uint8Array([66, 77]));
    expect(result.bmp.buffer).toBe(buffer);
  });
  it("accepts legacy images without inventing clocks or metadata", () => {
    expect(parsePreviewFrame(new Uint8Array([66, 77]).buffer).metadata).toBeNull();
    expect(parseFrameMetadata({ ...metadata, ts_dev: null, ts_host_ns: "-42" }).ts_dev).toBeNull();
  });
  it("refuses malformed, truncated and oversized headers before parsing their body", () => {
    expect(() => parsePreviewFrame(new Uint8Array([86, 70, 82, 77]).buffer)).toThrow(/header/);
    const packet = previewPacket();
    new DataView(packet).setUint32(4, 65537, true);
    expect(() => parsePreviewFrame(packet)).toThrow(/length/);
    expect(() => parsePreviewFrame(previewPacket().slice(0, -1))).toThrow();
    const invalid = previewPacket(); new Uint8Array(invalid)[8] = 255;
    expect(() => parsePreviewFrame(invalid)).toThrow();
  });
  it("rejects lossy numbers, invalid feature results and oversized data", () => {
    for (const extra of [
      { block_id: 9007199254740992 }, { block_id: "18446744073709551616" },
      { ts_dev: "-1" }, { ts_host_ns: "170141183460469231731687303715884105728" },
      { features: [{ name: "Bad", value: "x", error: "failed" }] },
      { features: [{ name: "Bad", value: null, error: null }] },
      { chunk_error: "x".repeat(1025) }, { chunks: Array(129).fill({ id: "1", bytes: 1 }) },
    ]) expect(() => parseFrameMetadata({ ...metadata, ...extra })).toThrow();
  });
});
