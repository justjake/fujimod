// Generic camera in the Fujifilm owner's-manual line-art style: uniform black
// outlines (contour 3, detail 2), flat light greys, no hands; red/blue solid arrows.
// Top view above, back view below, aligned like a technical drawing (front up).
import { ellipse, fill, group, path, rect, rgb, stroke, type Json } from './build.ts';

export type Pt = [number, number];

export const W = 960, H = 640;
export const INK = '#111111';
export const G = { body: '#d6d6d6', plate: '#ececec', light: '#f7f7f7', mid: '#bdbdbd', dark: '#8e8e8e', glass: '#a9a9a9', screen: '#c9c9c9' };
export const RED = '#d7000f', BLUE = '#0a6fd1';

export const EDGE = 5.5, PART = 2.25, DETAIL = 1.4;
const line = (w: number = PART) => stroke(rgb(INK), w);
/** Flat-filled outlined part; first listed is drawn on top within a layer. */
export const part = (nm: string, shapes: Json | Json[], colour: string, w: number = PART): Json => group(nm, [...([] as Json[]).concat(shapes), line(w), fill(rgb(colour))]);
export const ink = (nm: string, d: string, w: number = DETAIL): Json => group(nm, [...path(d), line(w)]);

// ---- layout: shared x positions between the views ----
export const X = { left: 196, right: 764, lensL: 268, lensR: 452, shoe: 404, speed: 548, shutter: 640, exp: 716 };
export const TOP = { y0: 150, y1: 252 }; // body depth in the top view; front edge at y0, rear edge at y1
export const BACK = { y0: 300, y1: 560 };
export const SHUTTER_TOP: Pt = [X.shutter, 206];
export const DPAD: Pt = [652, 436];

/** Short radial ticks around a circle (knurling, index marks). */
function ticks(cx: number, cy: number, r0: number, r1: number, n: number, from = 0, to = 360): string {
  let d = '';
  for (let i = 0; i < n; i++) {
    const a = ((from + ((to - from) * i) / n) * Math.PI) / 180;
    d += `M ${(cx + r0 * Math.cos(a)).toFixed(2)} ${(cy + r0 * Math.sin(a)).toFixed(2)} L ${(cx + r1 * Math.cos(a)).toFixed(2)} ${(cy + r1 * Math.sin(a)).toFixed(2)} `;
  }
  return d;
}
const roundedTop = (x0: number, y0: number, x1: number, y1: number, r: number): string => `M ${x0} ${y1} V ${y0 + r} C ${x0} ${y0 + r * 0.45}, ${x0 + r * 0.45} ${y0}, ${x0 + r} ${y0} H ${x1 - r} C ${x1 - r * 0.45} ${y0}, ${x1} ${y0 + r * 0.45}, ${x1} ${y0 + r} V ${y1} Z`;

export function topView(): Json[] {
  const { left, right } = X;
  const { y0, y1 } = TOP;
  const [sx, sy] = SHUTTER_TOP;
  return [
    // shutter release: button with its cable-release thread, in the power collar
    part('release-thread', ellipse(sx, sy, 8, 8), G.mid, DETAIL),
    part('shutter', ellipse(sx, sy, 26, 26), G.light),
    ink('collar-knurl', ticks(sx, sy, 17, 22, 28, 250, 470), DETAIL),
    part('collar', ellipse(sx, sy, 46, 46), G.plate),
    // exposure compensation dial
    ink('exp-index', ticks(X.exp, 204, 14, 22, 9, 200, 340), DETAIL),
    part('exp-cap', ellipse(X.exp, 204, 44, 44), G.light, DETAIL),
    ink('exp-knurl', ticks(X.exp, 204, 27, 32, 44), DETAIL),
    part('exp-dial', ellipse(X.exp, 204, 64, 64), G.plate),
    // shutter-speed dial
    ink('speed-index', ticks(X.speed, 204, 16, 24, 11, 190, 350), DETAIL),
    part('speed-cap', ellipse(X.speed, 204, 48, 48), G.light, DETAIL),
    ink('speed-knurl', ticks(X.speed, 204, 31, 36, 52), DETAIL),
    part('speed-dial', ellipse(X.speed, 204, 72, 72), G.plate),
    // hot shoe: rails and contacts
    group('shoe-contacts', [ellipse(X.shoe, 204, 7, 7), ellipse(X.shoe - 12, 196, 4, 4), ellipse(X.shoe + 12, 196, 4, 4), ellipse(X.shoe - 12, 212, 4, 4), ellipse(X.shoe + 12, 212, 4, 4), fill(rgb(G.dark))]),
    ink('shoe-rails', `M ${X.shoe - 32} 186 H ${X.shoe + 32} M ${X.shoe - 32} 222 H ${X.shoe + 32}`, DETAIL),
    part('shoe', rect(X.shoe - 40, 180, 80, 48, 4), G.light),
    // lens barrel toward the front (up): front rim, aperture ring, control ring
    ink('aperture-ridges', Array.from({ length: 16 }, (_, i) => `M ${X.lensL + 22 + i * ((X.lensR - X.lensL - 44) / 15)} ${y0 - 31} v 12`).join(' '), DETAIL),
    part('front-rim', rect(X.lensL + 10, y0 - 56, X.lensR - X.lensL - 20, 14, 3), G.mid, EDGE),
    part('aperture-ring', rect(X.lensL + 4, y0 - 42, X.lensR - X.lensL - 8, 30, 3), G.plate, EDGE),
    part('control-ring', rect(X.lensL, y0 - 14, X.lensR - X.lensL, 16, 3), G.mid, EDGE),
    part('lugs', [rect(left - 12, 188, 14, 24, 4), rect(right - 2, 188, 14, 24, 4)], G.mid, 3.5),
    // top plate with its chamfer line
    group('chamfer', [rect(left + 7, y0 + 7, right - left - 14, y1 - y0 - 14, 16), stroke(rgb(INK), 1.2)]),
    group('rear-shade', [rect(left + 3, y1 - 16, right - left - 6, 13, 6), fill(rgb(G.body))]),
    part('plate', rect(left, y0, right - left, y1 - y0, 22), G.plate, EDGE),
  ];
}

export function backView(): Json[] {
  const { left, right } = X;
  const { y0, y1 } = BACK;
  const [dx, dy] = DPAD;
  const keys: [number, number, string][] = [
    [612, 346, 'M -4 -5 L 6 0 L -4 5 Z'], // playback
    [692, 346, 'M -5 -4 H 5 M -3 -4 V 6 H 3 V -4 M -1 -6 H 1'], // delete
    [612, 528, 'M -6 0 C -6 -8, 6 -8, 6 0 C 6 8, -6 8, -6 0 Z M 3 3 L 7 7'], // Q
    [692, 528, 'M -6 -4 H 6 M -6 0 H 6 M -6 4 H 2'], // display
  ];
  return [
    ...['up', 'right', 'down', 'left'].map((nm, i) => {
      const a = (-90 + i * 90) * Math.PI / 180, r = 33, x = dx + r * Math.cos(a), y = dy + r * Math.sin(a);
      const t = (q: number) => [x + 5 * Math.cos(a + q), y + 5 * Math.sin(a + q)];
      return group(`dpad-${nm}-arrow`, [...path(`M ${x + 6 * Math.cos(a)} ${y + 6 * Math.sin(a)} L ${t(2.1).join(' ')} L ${t(-2.1).join(' ')} Z`), fill(rgb(G.dark))]);
    }),
    part('dpad-centre', ellipse(dx, dy, 36, 36), G.plate),
    ...['up', 'right', 'down', 'left'].map((nm, i) => ink(`dpad-${nm}`, segment(dx, dy, 18, 46, -135 + i * 90, -45 + i * 90), 1.5)),
    part('dpad', ellipse(dx, dy, 92, 92), G.light),
    part('dpad-ring', ellipse(dx, dy, 104, 104), G.mid),
    ...keys.flatMap(([x, y, icon], i) => [
      group(`key${i}-icon`, [...path(icon), stroke(rgb(G.dark), 1.6)], { p: [x, y] }),
      part(`key${i}`, ellipse(x, y, 30, 30), G.light),
    ]),
    part('led', ellipse(574, 334, 9, 9), G.mid, DETAIL),
    // grip ridge on the right
    ink('grip', `M ${right - 46} ${y0 + 40} V ${y1 - 30}`, DETAIL),
    // screen
    group('screen-glare', [...path('M 266 512 L 334 346 L 352 346 L 284 512 Z'), fill(rgb('#ffffff'), 40)]),
    part('screen', rect(238, 334, 296, 188, 6), G.screen, DETAIL),
    part('screen-frame', rect(226, 322, 320, 212, 12), G.dark),
    // top-plate band with rounded shoulders, body below
    ink('plate-seam', `M ${left + 3} ${y0 + 28} H ${right - 3}`, PART),
    group('plate-band', [...path(roundedTop(left, y0, right, y0 + 28, 26)), fill(rgb(G.plate))]),
    group('base-shade', [rect(left + 4, y1 - 22, right - left - 8, 18, 12), fill(rgb('#c9c9c9'))]),
    part('body', rect(left, y0, right - left, y1 - y0, 26), G.body, EDGE),
  ];
}

/** Ring segment between radii r0..r1 from angle a0 to a1 (degrees, y down). */
export function segment(cx: number, cy: number, r0: number, r1: number, a0: number, a1: number): string {
  const P = (r: number, a: number) => `${(cx + r * Math.cos((a * Math.PI) / 180)).toFixed(2)} ${(cy + r * Math.sin((a * Math.PI) / 180)).toFixed(2)}`;
  const k = (4 / 3) * Math.tan(((a1 - a0) * Math.PI) / 720);
  const C = (r: number, s: number, e: number) => {
    const sp = [cx + r * Math.cos((s * Math.PI) / 180), cy + r * Math.sin((s * Math.PI) / 180)];
    const ep = [cx + r * Math.cos((e * Math.PI) / 180), cy + r * Math.sin((e * Math.PI) / 180)];
    const ds = [-Math.sin((s * Math.PI) / 180) * r, Math.cos((s * Math.PI) / 180) * r];
    const de = [-Math.sin((e * Math.PI) / 180) * r, Math.cos((e * Math.PI) / 180) * r];
    const kk = e > s ? k : -k;
    return `C ${sp[0] + kk * ds[0]} ${sp[1] + kk * ds[1]} ${ep[0] - kk * de[0]} ${ep[1] - kk * de[1]} ${ep[0]} ${ep[1]}`;
  };
  return `M ${P(r1, a0)} ${C(r1, a0, a1)} L ${P(r0, a1)} ${C(r0, a1, a0)} Z`;
}

/** Bold arrow along a cubic: wide round-capped shaft, rounded head. Returns [head, shaft] groups. */
export function arrow(nm: string, [x0, y0]: Pt, [c1x, c1y]: Pt, [c2x, c2y]: Pt, [x1, y1]: Pt, colour: string, width = 12, extra: Json[] = []): [Json, Json] {
  const ang = Math.atan2(y1 - c2y, x1 - c2x);
  const hl = width * 2.1, hw = width * 1.55;
  const bx = x1 - Math.cos(ang) * hl, by = y1 - Math.sin(ang) * hl;
  const nx = -Math.sin(ang) * hw, ny = Math.cos(ang) * hw;
  const c = rgb(colour);
  return [
    group(nm + '-head', [...path(`M ${x1} ${y1} L ${bx + nx} ${by + ny} L ${bx - nx} ${by - ny} Z`), { ...stroke(c, width * 0.55), lj: 2 }, fill(c)]),
    group(nm, [...path(`M ${x0} ${y0} C ${c1x} ${c1y} ${c2x} ${c2y} ${bx + Math.cos(ang) * width * 0.4} ${by + Math.sin(ang) * width * 0.4}`), { ...stroke(c, width), lc: 2 }, ...extra]),
  ];
}

