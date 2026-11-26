import assert from "node:assert/strict";
import { test } from "node:test";
import { systemTourKeyAccent } from "./accentContrast.ts";

const fromHex = (hex) => [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16));

function luminance(rgb) {
  const [r, g, b] = rgb.map((value) => {
    const c = value / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

const accents = [
  "#000000", "#444444", "#808080", "#bbbbbb", "#ffffff", "#0078d4",
  "#005fb8", "#60cdff", "#ff0000", "#00ff00", "#0000ff", "#a83d83", "#ffff00",
];

for (const dark of [false, true]) {
  test("System tour keycap meets 4.5:1 in " + (dark ? "dark" : "light") + " mode", () => {
    for (const backdrop of [0, 255]) {
      const pill = (dark ? 44 : 249) * 0.78 + backdrop * 0.22;
      const keycap = pill * 0.88 + (dark ? 255 : 27) * 0.12;
      const background = luminance([keycap, keycap, keycap]);
      for (const accent of accents) {
        const shade = systemTourKeyAccent(accent, dark);
        const ink = luminance(fromHex(shade));
        const contrast = (Math.max(background, ink) + 0.05) / (Math.min(background, ink) + 0.05);
        assert.ok(contrast >= 4.5, accent + " on " + backdrop + " returned " + shade + " at " + contrast);
      }
    }
  });
}
