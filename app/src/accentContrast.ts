type RGB = [number, number, number];

function rgb(hex: string): RGB {
  const value = Number.parseInt(hex.slice(1), 16);
  return [value >> 16 & 255, value >> 8 & 255, value & 255];
}

function luminance([red, green, blue]: RGB): number {
  const linear = (channel: number) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
}

export function readableAccent(accent: string, surface: string, toward: 0 | 255, minimum = 4.5): string {
  const base = rgb(accent);
  const against = luminance(rgb(surface));
  const contrast = (color: RGB) => {
    const light = luminance(color);
    return (Math.max(light, against) + 0.05) / (Math.min(light, against) + 0.05);
  };
  if (contrast(base) >= minimum) return accent;

  let lo = 0;
  let hi = 256;
  let result: RGB = [toward, toward, toward];
  while (hi - lo > 1) {
    const step = (lo + hi) >> 1;
    const mix = (channel: number) => Math.round(channel + (toward - channel) * step / 256);
    const shade: RGB = [mix(base[0]), mix(base[1]), mix(base[2])];
    if (contrast(shade) < minimum) lo = step;
    else {
      hi = step;
      result = shade;
    }
  }
  return "#" + result.map((channel) => channel.toString(16).padStart(2, "0")).join("");
}

export function systemTourKeyAccent(accent: string, dark: boolean): string {
  // The keycap adds 12% pill ink over a 78% pill background. Over black in light
  // mode and white in dark mode, round the resulting gray toward lower contrast.
  return readableAccent(accent, dark ? "#6f6f6f" : "#aeaeae", dark ? 255 : 0, 4.55);
}
