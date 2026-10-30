import type { FrameMetadata } from "./types";

export const MAX_FRAME_METADATA_BYTES = 65536;
const MAX_ENTRIES = 128;
const MAX_TEXT = 1024;
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && [...value].length <= MAX_TEXT;
const optionalText = (value: unknown) => value == null || text(value);
const unsigned64 = (value: unknown) => typeof value === "string" && /^\d{1,20}$/.test(value) && BigInt(value) <= 18446744073709551615n;
const signed128 = (value: unknown) => typeof value === "string" && /^-?\d{1,39}$/.test(value)
  && BigInt(value) >= -(2n ** 127n) && BigInt(value) < 2n ** 127n;

/** Validate bounded wire data before displaying it; never round camera clocks in JS. */
export function parseFrameMetadata(value: unknown): FrameMetadata {
  if (!record(value) || !unsigned64(value.block_id)
      || !(value.ts_dev == null || unsigned64(value.ts_dev))
      || !(value.ts_host_ns == null || signed128(value.ts_host_ns))
      || typeof value.complete !== "boolean" || typeof value.truncated !== "boolean"
      || !optionalText(value.chunk_error)
      || !Array.isArray(value.chunks) || value.chunks.length > MAX_ENTRIES
      || !value.chunks.every(chunk => record(chunk) && unsigned64(chunk.id) && typeof chunk.bytes === "number" && Number.isSafeInteger(chunk.bytes) && chunk.bytes >= 0)
      || !Array.isArray(value.features) || value.features.length > MAX_ENTRIES
      || !value.features.every(feature => record(feature) && text(feature.name) && optionalText(feature.value) && optionalText(feature.error)
        && (feature.value != null) !== (feature.error != null))) {
    throw new Error("Invalid frame metadata");
  }
  return value as unknown as FrameMetadata;
}

/** Metadata and its BMP are one message, including when replaying a stopped frame. */
export function parsePreviewFrame(buffer: ArrayBuffer): { bmp: Uint8Array<ArrayBuffer>; metadata: FrameMetadata | null } {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 4 || String.fromCharCode(...bytes.subarray(0, 4)) !== "VFRM") {
    return { bmp: bytes, metadata: null };
  }
  if (bytes.length < 8) throw new Error("Incomplete preview frame header");
  const length = new DataView(buffer).getUint32(4, true);
  if (length < 2 || length > MAX_FRAME_METADATA_BYTES || length > bytes.length - 10) throw new Error("Invalid preview metadata length");
  const metadata = parseFrameMetadata(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(8, 8 + length))));
  const bmp = bytes.subarray(8 + length);
  if (bmp[0] !== 66 || bmp[1] !== 77) throw new Error("Invalid preview image signature");
  return { bmp, metadata };
}
