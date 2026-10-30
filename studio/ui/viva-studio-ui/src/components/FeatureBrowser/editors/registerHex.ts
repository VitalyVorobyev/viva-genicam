/** Bytes per row of a register's hex view. */
export const BYTES_PER_ROW = 16;

/**
 * The most bytes the hex view draws. The whole register is still read — and
 * copied to the editor — but a 100 000-byte file-access buffer drawn as 6 000
 * rows helps nobody.
 */
export const MAX_DISPLAY_BYTES = 4096;

export interface HexRow {
  /** Offset of the row's first byte, as eight hexadecimal digits. */
  offset: string;
  /** The row's bytes, two uppercase hexadecimal digits each. */
  hex: string[];
  /** The row's bytes as printable ASCII, `.` for anything else. */
  ascii: string;
}

function hexByte(byte: number): string {
  return byte.toString(16).toUpperCase().padStart(2, "0");
}

/** The first `limit` bytes of `bytes`, sixteen to a row. */
export function hexRows(bytes: ArrayLike<number>, limit = MAX_DISPLAY_BYTES): HexRow[] {
  const rows: HexRow[] = [];
  const end = Math.min(bytes.length, limit);
  for (let start = 0; start < end; start += BYTES_PER_ROW) {
    const hex: string[] = [];
    let ascii = "";
    for (let i = start; i < Math.min(start + BYTES_PER_ROW, end); i++) {
      const byte = bytes[i];
      hex.push(hexByte(byte));
      ascii += byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : ".";
    }
    rows.push({ offset: start.toString(16).toUpperCase().padStart(8, "0"), hex, ascii });
  }
  return rows;
}

/** `bytes` as editor text: space-separated pairs, sixteen to a line. */
export function toHexText(bytes: ArrayLike<number>): string {
  const lines: string[] = [];
  for (let start = 0; start < bytes.length; start += BYTES_PER_ROW) {
    const line: string[] = [];
    for (let i = start; i < Math.min(start + BYTES_PER_ROW, bytes.length); i++) {
      line.push(hexByte(bytes[i]));
    }
    lines.push(line.join(" "));
  }
  return lines.join("\n");
}

export type ParsedHex = { ok: true; bytes: number[] } | { ok: false; error: string };

/**
 * Bytes from hexadecimal text: pairs of digits, separated or not by
 * whitespace, `:`, `-` or `,`, with an optional leading `0x`. The count must
 * be exactly `length`: a register is written whole, never padded.
 */
export function parseHexBytes(text: string, length: number): ParsedHex {
  let digits = text.trim();
  if (/^0x/i.test(digits)) digits = digits.slice(2);
  digits = digits.replace(/[\s:,-]+/g, "");
  if (!/^[0-9a-fA-F]*$/.test(digits)) {
    return { ok: false, error: "Only hexadecimal digits, spaces, ':', '-' and ',' are accepted." };
  }
  if (digits.length % 2 !== 0) {
    return { ok: false, error: "An odd number of hex digits: each byte is two digits." };
  }
  const count = digits.length / 2;
  if (count !== length) {
    return { ok: false, error: `${count} of ${length} bytes: the register is written whole.` };
  }
  const bytes: number[] = [];
  for (let i = 0; i < digits.length; i += 2) {
    bytes.push(Number.parseInt(digits.slice(i, i + 2), 16));
  }
  return { ok: true, bytes };
}

/** A register's length, from its live state's `{ kind: "register", length }`. */
export function registerLength(value: unknown): number | null {
  if (value && typeof value === "object" && "length" in value) {
    const length = (value as { length: unknown }).length;
    if (typeof length === "number" && Number.isInteger(length) && length >= 0) return length;
  }
  return null;
}
