// Small Lottie (Bodymovin 5.x) builder for the setup wizard's illustrations.
// Scenes in this directory describe the cartoon camera with these helpers; the
// output is plain Lottie JSON, verified in the Skia Skottie player (text-to-lottie).

export const FPS = 60;

/** Lottie JSON objects (shapes, layers, properties) are plain records. */
export type Json = { [key: string]: unknown };
export type RGBA = [number, number, number, number];
type Bezier = [number, number, number, number];
export type Ease = keyof typeof EASE | Bezier;
/** A static value, or a { a, k } property already built. */
type Value = number | number[] | Json;
/** One animation step: frame, value and the curve used to travel to the next step. */
export type Step = [number, number | number[], Ease?];

/** Colour from '#rrggbb' to Lottie's 0..1 RGBA. */
export function rgb(hex: string, a = 1): RGBA {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, a];
}

// Easing anchors from the text-to-lottie motion guide (cubic-bezier x1,y1,x2,y2).
export const EASE: Record<string, Bezier | null> = {
  sharp: [0.2, 0.75, 0.34, 0.94],
  settle: [0, 0.65, 0.51, 0.99],
  travel: [1, 0.49, 0, 0.55],
  exit: [1, 0.02, 0.54, 0.42],
  pop: [0.94, 0.75, 0.34, 0.94],
  linear: [0, 0, 1, 1],
  hold: null,
};

const vec = (v: number | number[]): number[] => (Array.isArray(v) ? v : [v]);
/** Static property. */
export const st = (v: unknown): Json => ({ a: 0, k: v });
/**
 * Animated property from [frame, value, ease?] steps; `ease` is the curve used to
 * travel from this step to the next (default travel). 'hold' jumps.
 */
export function an(steps: Step[]): Json {
  if (steps.length === 1) return st(steps[0][1]);
  const k = steps.map(([t, v, ease = 'travel'], i) => {
    const kf: Json = { t, s: vec(v) };
    if (i < steps.length - 1) {
      if (ease === 'hold') kf.h = 1;
      else {
        const curve = typeof ease === 'string' ? EASE[ease] : ease;
        if (!curve) throw Error(`an: unknown ease ${String(ease)}`);
        const [x1, y1, x2, y2] = curve;
        const n = vec(v).length;
        kf.o = { x: Array(n).fill(x1), y: Array(n).fill(y1) };
        kf.i = { x: Array(n).fill(x2), y: Array(n).fill(y2) };
      }
    }
    return kf;
  });
  return { a: 1, k };
}
const prop = (v: Value): Json => (v && typeof v === 'object' && !Array.isArray(v) && 'a' in v ? v : st(v));

export interface Transform { a?: Value; p?: Value; s?: Value; r?: Value; o?: Value }
export function transform({ a = [0, 0], p = [0, 0], s = [100, 100], r = 0, o = 100 }: Transform = {}): Json {
  return { ty: 'tr', a: prop(a), p: prop(p), s: prop(s), r: prop(r), o: prop(o), sk: st(0), sa: st(0) };
}
export const group = (nm: string, items: Json[], tr: Transform = {}): Json => ({ ty: 'gr', nm, it: [...items, transform(tr)] });
export const rect = (x: number, y: number, w: number, h: number, r = 0): Json => ({ ty: 'rc', nm: 'rect', p: st([x + w / 2, y + h / 2]), s: st([w, h]), r: st(r) });
export const ellipse = (cx: number, cy: number, w: number, h: number): Json => ({ ty: 'el', nm: 'ellipse', p: st([cx, cy]), s: st([w, h]) });
export const fill = (c: Value, o: Value = 100): Json => ({ ty: 'fl', nm: 'fill', c: prop(c), o: prop(o), r: 1 });
export const stroke = (c: Value, w: Value, o: Value = 100): Json => ({ ty: 'st', nm: 'stroke', c: prop(c), o: prop(o), w: prop(w), lc: 2, lj: 2, ml: 4 });
/** Linear gradient fill from (x1,y1) to (x2,y2) through [offset, '#hex'] stops. */
export function gradient(x1: number, y1: number, x2: number, y2: number, stops: [number, string][]): Json {
  const k = stops.flatMap(([o, hex]) => [o, ...rgb(hex).slice(0, 3)]);
  return { ty: 'gf', nm: 'gradient', o: st(100), r: 1, s: st([x1, y1]), e: st([x2, y2]), t: 1, g: { p: stops.length, k: st(k) } };
}
export const trim = (s: Value, e: Value, o: Value = 0): Json => ({ ty: 'tm', nm: 'trim', s: prop(s), e: prop(e), o: prop(o), m: 1 });

/** SVG path data (absolute or relative M/L/H/V/C/S/Q/Z) to Lottie bezier shapes. */
interface Bez { v: number[][]; i: number[][]; o: number[][]; c: boolean }
export function path(d: string): Json[] {
  const tokens = d.match(/[a-zA-Z]|-?\d*\.?\d+(?:e-?\d+)?/g) ?? [];
  const shapes: Bez[] = [];
  let cur: Bez = { v: [], i: [], o: [], c: false },
    x = 0,
    y = 0,
    lastCtrl: number[] | null = null,
    cmd = '';
  let i = 0;
  const num = () => parseFloat(tokens[i++]);
  const start = (px: number, py: number) => {
    cur = { v: [[px, py]], i: [[0, 0]], o: [[0, 0]], c: false };
    shapes.push(cur);
  };
  const line = (px: number, py: number) => {
    cur.v.push([px, py]);
    cur.i.push([0, 0]);
    cur.o.push([0, 0]);
  };
  const cubic = (c1x: number, c1y: number, c2x: number, c2y: number, px: number, py: number) => {
    const last = cur.v.length - 1;
    cur.o[last] = [c1x - cur.v[last][0], c1y - cur.v[last][1]];
    cur.v.push([px, py]);
    cur.i.push([c2x - px, c2y - py]);
    cur.o.push([0, 0]);
  };
  while (i < tokens.length) {
    if (/[a-zA-Z]/.test(tokens[i])) cmd = tokens[i++];
    const rel = cmd === cmd.toLowerCase() && cmd !== 'z' ? 1 : 0;
    const ox = rel ? x : 0,
      oy = rel ? y : 0;
    switch (cmd.toUpperCase()) {
      case 'M':
        x = ox + num();
        y = oy + num();
        start(x, y);
        cmd = rel ? 'l' : 'L';
        lastCtrl = null;
        break;
      case 'L':
        x = ox + num();
        y = oy + num();
        line(x, y);
        lastCtrl = null;
        break;
      case 'H':
        x = ox + num();
        line(x, y);
        lastCtrl = null;
        break;
      case 'V':
        y = oy + num();
        line(x, y);
        lastCtrl = null;
        break;
      case 'C': {
        const c1x = ox + num(), c1y = oy + num(), c2x = ox + num(), c2y = oy + num();
        x = ox + num();
        y = oy + num();
        cubic(c1x, c1y, c2x, c2y, x, y);
        lastCtrl = [c2x, c2y];
        break;
      }
      case 'S': {
        const c1x = lastCtrl ? 2 * x - lastCtrl[0] : x, c1y = lastCtrl ? 2 * y - lastCtrl[1] : y;
        const c2x = ox + num(), c2y = oy + num();
        x = ox + num();
        y = oy + num();
        cubic(c1x, c1y, c2x, c2y, x, y);
        lastCtrl = [c2x, c2y];
        break;
      }
      case 'Q': {
        const qx = ox + num(), qy = oy + num();
        const px = ox + num(), py = oy + num();
        cubic(x + (2 / 3) * (qx - x), y + (2 / 3) * (qy - y), px + (2 / 3) * (qx - px), py + (2 / 3) * (qy - py), px, py);
        x = px;
        y = py;
        lastCtrl = null;
        break;
      }
      case 'Z': {
        // close: drop a duplicated end vertex and keep its in-tangent
        const n = cur.v.length - 1;
        if (n > 0 && Math.hypot(cur.v[n][0] - cur.v[0][0], cur.v[n][1] - cur.v[0][1]) < 0.01) {
          cur.i[0] = cur.i[n];
          cur.v.pop();
          cur.i.pop();
          cur.o.pop();
        }
        cur.c = true;
        x = cur.v[0][0];
        y = cur.v[0][1];
        lastCtrl = null;
        break;
      }
      default:
        throw Error(`path: unsupported command ${cmd}`);
    }
  }
  return shapes.map((ks) => ({ ty: 'sh', nm: 'path', ks: st(ks) }));
}

/** A filled + outlined piece: shapes, fill (colour or gradient), ink outline. */
export function piece(nm: string, shapes: Json | Json[], paint: string | Json | null, { line = 4, ink = '#14131a', tr = {} }: { line?: number; ink?: string; tr?: Transform } = {}): Json {
  const fills = paint ? [typeof paint === 'string' ? fill(rgb(paint)) : paint] : [];
  const outline = line ? [stroke(rgb(ink), line)] : [];
  return group(nm, [...([] as Json[]).concat(shapes), ...outline, ...fills], tr);
}

export interface Layer extends Json { ind: number; ip: number; op?: number; shapes?: Json[] }
interface LayerOptions { ks?: Transform; parent?: number; ip?: number; op?: number; ind?: number }
let nextLayer = 1;
/** Shape layer; `items` are shape groups, listed top-most first. */
export function layer(nm: string, items: Json[], { ks = {}, parent, ip = 0, op, ind }: LayerOptions = {}): Layer {
  const l: Layer = {
    ddd: 0,
    ind: ind ?? nextLayer++,
    ty: 4,
    nm,
    sr: 1,
    ks: { a: prop(ks.a ?? [0, 0, 0]), p: prop(ks.p ?? [0, 0, 0]), s: prop(ks.s ?? [100, 100, 100]), r: prop(ks.r ?? 0), o: prop(ks.o ?? 100) },
    ao: 0,
    shapes: items,
    ip,
    op,
    st: 0,
    bm: 0,
  };
  if (parent) l.parent = parent;
  return l;
}
/** Null layer used as a camera / pivot. */
export function nul(nm: string, ks: Transform, { parent, op, ind }: LayerOptions = {}): Layer {
  const l: Layer = { ddd: 0, ind: ind ?? nextLayer++, ty: 3, nm, sr: 1, ks: { a: prop(ks.a ?? [0, 0, 0]), p: prop(ks.p ?? [0, 0, 0]), s: prop(ks.s ?? [100, 100, 100]), r: prop(ks.r ?? 0), o: prop(ks.o ?? 100) }, ao: 0, ip: 0, op, st: 0, bm: 0 };
  if (parent) l.parent = parent;
  return l;
}

export function scene({ nm, w, h, op, layers, markers = [] }: { nm: string; w: number; h: number; op: number; layers: Layer[]; markers?: Json[] }): Json {
  return { v: '5.12.0', fr: FPS, ip: 0, op, w, h, nm, ddd: 0, assets: [], markers, layers };
}
