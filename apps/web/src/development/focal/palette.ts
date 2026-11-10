/** The grays a Focal screen draws with. BACKGROUND COLOR tints every gray with one
 * hue at that gray's own value (HSV), so the bevel and focus bar keep their contrast. */
export const GRAY = { page: 15, group: 25, hairline: 40, focus: 58 } as const;
export const TINT_SATURATION = 0.6;

export type Palette = Record<keyof typeof GRAY, string>;

function hsv(hue: number, saturation: number, value: number): string {
  const f = (n: number) => {
    const k = (n + hue / 60) % 6;
    return value - value * saturation * Math.max(0, Math.min(k, 4 - k, 1));
  };
  const channel = (n: number) => Math.round(f(n) * 255);
  return `rgb(${channel(5)} ${channel(3)} ${channel(1)})`;
}

export function palette(hue: number | null): Palette {
  const tint = (level: number) =>
    hue === null ? `rgb(${level} ${level} ${level})` : hsv(hue, TINT_SATURATION, level / 255);
  return {
    page: tint(GRAY.page),
    group: tint(GRAY.group),
    hairline: tint(GRAY.hairline),
    focus: tint(GRAY.focus),
  };
}

/** A hue drawn bright enough to read (the page shows it at its own dark value);
 * no hue is the matching gray. */
export function hueColor(hue: number | null): string {
  return hue === null ? hsv(0, 0, 0.62) : hsv(hue, 0.7, 0.62);
}
