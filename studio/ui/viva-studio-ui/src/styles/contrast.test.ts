import { describe, expect, it } from "vitest";
import tokens from "./tokens.css?raw";

function token(name: string): string {
  const match = tokens.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  if (!match) throw new Error(`no --${name} in tokens.css`);
  return match[1];
}

/** WCAG 2 relative luminance of a `#rrggbb` colour. */
function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// WCAG AA asks 4.5:1 of text this size. `--text-muted` labels hints, units
// and counts in about 80 rules, and at #484f58 it reached 2.3:1 (ST-40).
describe("text tokens", () => {
  for (const text of ["text-secondary", "text-muted"]) {
    for (const background of ["bg-base", "bg-surface", "bg-elevated"]) {
      it(`--${text} on --${background} meets WCAG AA`, () => {
        expect(contrast(token(text), token(background))).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});
