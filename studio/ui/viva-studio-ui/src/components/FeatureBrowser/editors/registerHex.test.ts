import { describe, expect, it } from "vitest";
import { hexRows, parseHexBytes, registerLength, toHexText } from "./registerHex";

describe("hexRows", () => {
  it("lays bytes out sixteen to a row with offsets and ASCII", () => {
    const bytes = [...Array.from("Viva GenICam!!!!", (c) => c.charCodeAt(0)), 0x00, 0xff];
    const rows = hexRows(bytes);
    expect(rows).toHaveLength(2);
    expect(rows[0].offset).toBe("00000000");
    expect(rows[0].ascii).toBe("Viva GenICam!!!!");
    expect(rows[1]).toEqual({ offset: "00000010", hex: ["00", "FF"], ascii: ".." });
  });

  it("draws no more than its limit", () => {
    const rows = hexRows(new Uint8Array(100), 32);
    expect(rows).toHaveLength(2);
    expect(rows[1].offset).toBe("00000010");
  });
});

describe("parseHexBytes", () => {
  it("accepts separators and a 0x prefix", () => {
    expect(parseHexBytes("0x01 02:0a-FF", 4)).toEqual({ ok: true, bytes: [1, 2, 10, 255] });
    expect(parseHexBytes("0102\n0304", 4)).toEqual({ ok: true, bytes: [1, 2, 3, 4] });
  });

  it("refuses anything but exactly the register's length", () => {
    const short = parseHexBytes("01 02", 4);
    expect(short.ok).toBe(false);
    expect(!short.ok && short.error).toContain("2 of 4 bytes");
    expect(parseHexBytes("01 02 03 04 05", 4).ok).toBe(false);
    expect(parseHexBytes("012", 2).ok).toBe(false);
    expect(parseHexBytes("zz", 1).ok).toBe(false);
  });

  it("reads back what toHexText writes", () => {
    const bytes = Array.from({ length: 20 }, (_, i) => i * 13);
    expect(parseHexBytes(toHexText(bytes), 20)).toEqual({ ok: true, bytes });
  });
});

it("reads a register's length from its live value", () => {
  expect(registerLength({ kind: "register", length: 16 })).toBe(16);
  expect(registerLength(null)).toBeNull();
  expect(registerLength({ kind: "register" })).toBeNull();
});
