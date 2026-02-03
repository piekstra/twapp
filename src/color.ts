/**
 * Color utilities for deriving theme shades from accent color.
 */

export function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.substring(0, 2), 16),
    parseInt(h.substring(2, 4), 16),
    parseInt(h.substring(4, 6), 16),
  ];
}

export function rgbToHex(r: number, g: number, b: number): string {
  return (
    "#" +
    [r, g, b].map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("")
  );
}

export function adjustBrightness(hex: string, amount: number): string {
  const [r, g, b] = hexToRgb(hex);
  return rgbToHex(r + amount, g + amount, b + amount);
}

export function applyThemeColor(color: string) {
  const style = document.documentElement.style;
  style.setProperty("--bg-secondary", color);
  style.setProperty("--border-color", adjustBrightness(color, -20));
  style.setProperty("--border-hover", adjustBrightness(color, -40));
  style.setProperty("--scrollbar-thumb", adjustBrightness(color, -30));
  style.setProperty("--scrollbar-thumb-hover", adjustBrightness(color, -50));
}
