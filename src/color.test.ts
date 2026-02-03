import { describe, it, expect } from "vitest";
import { hexToRgb, rgbToHex, adjustBrightness } from "./color";

describe("hexToRgb", () => {
  it("converts black", () => {
    expect(hexToRgb("#000000")).toEqual([0, 0, 0]);
  });

  it("converts white", () => {
    expect(hexToRgb("#ffffff")).toEqual([255, 255, 255]);
  });

  it("converts red", () => {
    expect(hexToRgb("#ff0000")).toEqual([255, 0, 0]);
  });

  it("handles without hash prefix", () => {
    expect(hexToRgb("ff0000")).toEqual([255, 0, 0]);
  });

  it("converts a theme color", () => {
    expect(hexToRgb("#ffe0e0")).toEqual([255, 224, 224]);
  });
});

describe("rgbToHex", () => {
  it("converts black", () => {
    expect(rgbToHex(0, 0, 0)).toBe("#000000");
  });

  it("converts white", () => {
    expect(rgbToHex(255, 255, 255)).toBe("#ffffff");
  });

  it("converts mid values", () => {
    expect(rgbToHex(128, 64, 32)).toBe("#804020");
  });

  it("clamps values above 255", () => {
    expect(rgbToHex(300, 256, 999)).toBe("#ffffff");
  });

  it("clamps values below 0", () => {
    expect(rgbToHex(-10, -1, -255)).toBe("#000000");
  });

  it("rounds fractional values", () => {
    expect(rgbToHex(127.6, 0.4, 255)).toBe("#8000ff");
  });
});

describe("adjustBrightness", () => {
  it("makes lighter with positive amount", () => {
    const result = adjustBrightness("#808080", 50);
    expect(hexToRgb(result)).toEqual([178, 178, 178]);
  });

  it("makes darker with negative amount", () => {
    const result = adjustBrightness("#808080", -50);
    expect(hexToRgb(result)).toEqual([78, 78, 78]);
  });

  it("clamps to white when exceeding 255", () => {
    const result = adjustBrightness("#f0f0f0", 100);
    expect(result).toBe("#ffffff");
  });

  it("clamps to black when going below 0", () => {
    const result = adjustBrightness("#101010", -50);
    expect(result).toBe("#000000");
  });

  it("no change with zero amount", () => {
    expect(adjustBrightness("#abcdef", 0)).toBe("#abcdef");
  });
});
