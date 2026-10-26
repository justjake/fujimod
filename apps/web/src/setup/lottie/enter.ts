// Step 1, "Start in service mode", on the X70 drawn as owner's-manual line art
// (art/x70_layers.py + art/pack.py): cable in, switch OFF, hold UP + shutter, switch ON.
// Motion follows the text-to-lottie motion guide: one camera move at a time with travel
// easing and a soft settle, arrows drawn on with trim paths, no bounce.
import { an, ellipse, fill, group, layer, nul, path, rect, rgb, scene, st, stroke, trim, type Json, type Layer, type Step } from './build.ts';
import art from '../art/x70-rear34.json' with { type: 'json' };

type Pt = [number, number];
interface Image { x: number; y: number; w: number; h: number; px: number[]; data: string }
interface Lines { width: number; paths: number[][][] }
interface Art { anchors: { size: number[]; shutter: number[]; pivot: number[]; up: number[]; dpad: number[]; port: number[]; screen: number[][]; lever: number[][] }; body: { image: Image; lines: Lines[] }; lever: { image: Image; lines: Lines[] }[] }
const A = art as Art;
const [W, H] = A.anchors.size;
const pt = (v: number[]): Pt => [v[0], v[1]];
const SHUTTER = pt(A.anchors.shutter), PIVOT = pt(A.anchors.pivot), UP = pt(A.anchors.up), PORT = pt(A.anchors.port);
const BG = '#f5f5f7', RED = '#d7000f', BLUE = '#0a6fd1', INK = '#111111';
const DRAW: [number, number, number, number] = [0.4, 0, 0.2, 1];

// ---------- timing (frames at 60 fps) ----------
const T = {
  b0: 0, cable: [44, 104],
  b1: 132, toTop: [132, 196], arrowOff: [204, 240], leverOff: [214, 246], screenOff: [234, 264], arrowOffOut: [280, 300],
  b2: 304, toHold: [304, 368], ringShutter: [376, 406], ringUp: [418, 448],
  b3: 480, toTop2: [480, 540], arrowOn: [548, 584], leverOn: [558, 590], arrowOnOut: [616, 636],
  b4: 640, toAll: [640, 704], release: [716, 746],
  fade: [868, 896], op: 900,
} as const;

// ---------- the viewer's camera ----------
interface View { a: number[]; p: number[]; s: number }
const ALL: View = { a: [W / 2, H / 2, 0], p: [W / 2, H / 2, 0], s: 100 };
const TOPV: View = { a: [SHUTTER[0] - 40, SHUTTER[1] + 40, 0], p: [W / 2, H * 0.46, 0], s: 175 };
const HOLDV: View = { a: [(SHUTTER[0] + UP[0]) / 2 - 40, (SHUTTER[1] + UP[1]) / 2, 0], p: [W / 2, H / 2, 0], s: 140 };
const moves: [number, View][] = [[0, ALL], [T.toTop[0], ALL], [T.toTop[1], TOPV], [T.toHold[0], TOPV], [T.toHold[1], HOLDV], [T.toTop2[0], HOLDV], [T.toTop2[1], TOPV], [T.toAll[0], TOPV], [T.toAll[1], ALL]];
const track = (value: (v: View) => number[]): Json =>
  an(moves.map(([t, v], i): Step => [t, value(v), i === moves.length - 1 || i % 2 === 0 ? 'hold' : 'travel']));
const cam = nul('camera', {
  a: track((v) => v.a),
  p: track((v) => v.p),
  s: an([[0, [96, 96, 100], 'settle'], [44, [100, 100, 100], 'hold'], ...moves.slice(1).map(([t, v], i): Step => [t, [v.s, v.s, 100], i % 2 === 0 ? 'travel' : 'hold'])]),
  o: an([[0, 0, 'settle'], [30, 100, 'hold'], [T.fade[0], 100, 'exit'], [T.fade[1], 0]]),
}, { op: T.op });
const P = { parent: cam.ind, op: T.op };

// ---------- art: fill images under vector line art ----------
const assets: Json[] = [];
function imageLayer(nm: string, im: Image, ks: Json = {}): Layer {
  const id = `img-${assets.length}`;
  assets.push({ id, w: im.px[0], h: im.px[1], u: '', p: im.data, e: 1 });
  return { ddd: 0, ind: 0, ty: 2, nm, refId: id, sr: 1, ks: { a: st([0, 0, 0]), p: st([im.x, im.y, 0]), s: st([50, 50, 100]), r: st(0), o: st(100), ...ks }, ao: 0, ip: 0, op: T.op, st: 0, bm: 0, parent: cam.ind } as Layer;
}
const lineGroups = (ls: Lines[]): Json[] => ls.map((g, i) => group(`w${g.width}-${i}`, [
  ...g.paths.map((p) => ({ ty: 'sh', nm: 'stroke', ks: st({ c: false, v: p, i: p.map(() => [0, 0]), o: p.map(() => [0, 0]) }) })),
  { ...stroke(rgb(INK), g.width), lc: 2, lj: 2 },
]));
let nextInd = 1000;
const withInd = (l: Layer): Layer => ({ ...l, ind: nextInd++ });
const bodyImage = withInd(imageLayer('body-fill', A.body.image));
const bodyLines = layer('body-lines', lineGroups(A.body.lines), P);

// power lever: one rendered step per layer, shown in turn as the lever moves (hold keys)
const ease = (x: number) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);
function leverAt(t: number): number {
  const seg = (a: number, b: number) => ease(Math.min(1, Math.max(0, (t - a) / (b - a))));
  return t < T.leverOn[0] ? seg(T.leverOff[0], T.leverOff[1]) : 1 - seg(T.leverOn[0], T.leverOn[1]);
}
const STEPS = A.lever.length;
const leverLayers = A.lever.flatMap((step, k) => {
  const shown: Step[] = [];
  let prev = -1;
  for (let t = 0; t < T.op; t++) {
    const on = Math.round(leverAt(t) * (STEPS - 1)) === k ? 100 : 0;
    if (on !== prev) shown.push([t, on, 'hold']);
    prev = on;
  }
  const o = shown.length > 1 ? an(shown) : st(shown[0][1]);
  return [layer(`lever-${k}-lines`, lineGroups(step.lines), { ...P, ks: { o } }), withInd(imageLayer(`lever-${k}-fill`, step.image, { o }))];
});

// ---------- overlays ----------
const ARROW_W = 20;
const HALO = 9; // white keyline either side, so arrows read over line art
/** Bold arrow along points: round-capped shaft, rounded head, drawn on over [t0, t1]. */
function arrow(nm: string, pts: Pt[], colour: string, [t0, t1]: readonly number[], [o0, o1]: readonly number[]): Layer {
  const n = pts.length, [x1, y1] = pts[n - 1], [xp, yp] = pts[n - 2];
  const ang = Math.atan2(y1 - yp, x1 - xp), hl = ARROW_W * 2.1, hw = ARROW_W * 1.55;
  const bx = x1 - Math.cos(ang) * hl, by = y1 - Math.sin(ang) * hl, nx = -Math.sin(ang) * hw, ny = Math.cos(ang) * hw;
  const shaft = [...pts.slice(0, -1), [bx + Math.cos(ang) * ARROW_W * 0.4, by + Math.sin(ang) * ARROW_W * 0.4]];
  const smooth = catmull(shaft);
  const c = rgb(colour), white = rgb('#ffffff');
  const head = path(`M ${x1} ${y1} L ${bx + nx} ${by + ny} L ${bx - nx} ${by - ny} Z`);
  const headO = an([[0, 0, 'hold'], [t1 - 8, 0, 'linear'], [t1 - 2, 100]]);
  const drawOn = () => trim(0, an([[t0, 0, DRAW], [t1, 100]]));
  return layer(nm, [
    group('head', [...head, { ...stroke(c, ARROW_W * 0.5), lj: 2 }, fill(c)], { o: headO }),
    group('head-halo', [...head, { ...stroke(white, ARROW_W * 0.5 + HALO * 2), lj: 2 }], { o: headO }),
    group('shaft', [...path(smooth), { ...stroke(c, ARROW_W), lc: 2 }, drawOn()]),
    group('shaft-halo', [...path(smooth), { ...stroke(white, ARROW_W + HALO * 2), lc: 2 }, drawOn()]),
  ], { ...P, ip: t0 + 2, op: o1 + 1, ks: { o: an([[t0 + 2, 0, 'linear'], [t0 + 6, 100, 'hold'], [o0, 100, 'exit'], [o1, 0]]) } });
}
/** Smooth path through points (Catmull-Rom as cubic Beziers). */
function catmull(p: number[][]): string {
  let d = `M ${p[0][0]} ${p[0][1]}`;
  for (let i = 0; i < p.length - 1; i++) {
    const a = p[Math.max(0, i - 1)], b = p[i], c = p[i + 1], e = p[Math.min(p.length - 1, i + 2)];
    d += ` C ${b[0] + (c[0] - a[0]) / 6} ${b[1] + (c[1] - a[1]) / 6} ${c[0] - (e[0] - b[0]) / 6} ${c[1] - (e[1] - b[1]) / 6} ${c[0]} ${c[1]}`;
  }
  return d;
}
// red: an arc around the shutter collar in the direction the lever travels. The collar is
// seen at an angle, so the arc is the lever's own path (an ellipse about the shutter axis)
// widened to sit outside the collar and extended past both ends of the lever's travel.
const L0 = A.anchors.lever[0], L1 = A.anchors.lever[A.anchors.lever.length - 1];
function collarArc(fromEnd: boolean): Pt[] {
  // fit the ellipse through the lever path: centre PIVOT, axes from the sampled positions
  const ang = (p: number[]) => Math.atan2((p[1] - PIVOT[1]) / 0.42, p[0] - PIVOT[0]);
  const a0 = ang(L0), a1 = ang(L1), r = Math.hypot(L0[0] - PIVOT[0], (L0[1] - PIVOT[1]) / 0.42) * 2.6;
  const span = a1 - a0, start = a0 - span * 1.6, end = a1 + span * 1.6;
  const pts: Pt[] = Array.from({ length: 9 }, (_, i) => {
    const t = start + ((end - start) * i) / 8;
    return [PIVOT[0] + r * Math.cos(t), PIVOT[1] - 70 + 0.42 * r * Math.sin(t)];
  });
  return fromEnd ? pts.reverse() : pts;
}
const offArrow = arrow('arrow-off', collarArc(false), RED, T.arrowOff, T.arrowOffOut);
const onArrow = arrow('arrow-on', collarArc(true), RED, T.arrowOn, T.arrowOnOut);

// blue: press and hold — arrow onto the key, ring around it, then a gentle breathing ring
function hold(nm: string, [cx, cy]: Pt, rx: number, ry: number, from: Pt[], [t0, t1]: readonly number[]): Layer[] {
  // the arrow points the key out, then leaves the ring to show it is held
  const a = arrow(nm + '-arrow', [...from, [cx + rx * 0.7, cy - ry * 0.7]], BLUE, [t0, t1], [t1 + 46, t1 + 66]);
  const draw = trim(0, an([[t1 - 6, 0, DRAW], [t1 + 22, 100]]));
  const beats = [0, 1, 2, 3].map((n) => t1 + 22 + n * 60);
  const ring = layer(nm + '-ring', [
    group('ring', [ellipse(cx, cy, rx * 2, ry * 2), { ...stroke(rgb(BLUE), 9), lc: 2 }, draw]),
    group('ring-halo', [ellipse(cx, cy, rx * 2, ry * 2), { ...stroke(rgb('#ffffff'), 9 + 2 * HALO), lc: 2 }, trim(0, an([[t1 - 6, 0, DRAW], [t1 + 22, 100]]))]),
    group('breathe', [ellipse(cx, cy, rx * 2, ry * 2), stroke(rgb(BLUE), 4)], {
      a: [cx, cy], p: [cx, cy],
      s: an(beats.flatMap((b, i): Step[] => [[b, [100, 100], 'linear'], [b + 59, [130, 130], i === beats.length - 1 ? 'linear' : 'hold']])),
      o: an(beats.flatMap((b, i): Step[] => [[b, 55, 'linear'], [b + 59, 0, i === beats.length - 1 ? 'linear' : 'hold']])),
    }),
  ], { ...P, ip: t1 - 7, op: T.release[1] + 1, ks: { o: an([[T.release[0], 100, 'exit'], [T.release[1], 0]]) } });
  return [a, ring];
}
const holdShutter = hold('hold-shutter', SHUTTER, 34, 19, [[SHUTTER[0] + 70, SHUTTER[1] - 190], [SHUTTER[0] + 40, SHUTTER[1] - 110]], T.ringShutter);
const holdUp = hold('hold-up', UP, 30, 20, [[UP[0] + 230, UP[1] - 60], [UP[0] + 130, UP[1] - 40]], T.ringUp);

// the screen is lit until the camera switches off
const [s0, s1, s2, s3] = A.anchors.screen;
const screenOn = layer('screen-on', [group('picture', [
  ...path(`M ${s0[0]} ${s0[1]} L ${s1[0]} ${s1[1]} L ${s2[0]} ${s2[1]} L ${s3[0]} ${s3[1]} Z`),
  { ty: 'gf', nm: 'picture-fill', o: st(100), r: 1, s: st(s1), e: st(s3), t: 1, g: { p: 3, k: st([0, 0.66, 0.78, 0.88, 0.55, 0.88, 0.82, 0.68, 1, 0.58, 0.69, 0.58]) } },
])], { ...P, ks: { o: an([[T.screenOff[0], 100, 'exit'], [T.screenOff[1], 0, 'hold'], [T.op - 2, 0, 'hold'], [T.op - 1, 100]]) } });

// USB cable into the side door: round-capped lead with an outlined plug
const lead = `M ${PORT[0] + 40} ${PORT[1]} C ${PORT[0] + 140} ${PORT[1] + 10}, ${PORT[0] + 150} ${PORT[1] + 200}, ${PORT[0] + 420} ${PORT[1] + 280}`;
const cable = layer('cable', [
  group('plug', [rect(PORT[0] + 4, PORT[1] - 16, 44, 32, 7), stroke(rgb(INK), 2.4), fill(rgb('#f2f2f2'))]),
  group('lead-fill', [...path(lead), { ...stroke(rgb('#e6e6e6'), 11), lc: 2 }]),
  group('lead-edge', [...path(lead), { ...stroke(rgb(INK), 15.8), lc: 2 }]),
], { ...P, ks: { p: an([[0, [380, 90, 0], 'hold'], [T.cable[0], [380, 90, 0], [0.3, 0, 0, 1]], [T.cable[1], [0, 0, 0]]]) } });

const bg = layer('background', [group('bg', [rect(0, 0, W, H, 0), { ty: 'fl', nm: 'fill', c: { sid: 'bgColor', a: 0, k: rgb(BG) }, o: st(100), r: 1 }])], { op: T.op });

const layers: Layer[] = [offArrow, onArrow, ...holdShutter, ...holdUp, ...leverLayers, screenOn, bodyLines, bodyImage, cable, cam, bg];
for (const l of layers) l.op ??= T.op;

const beatsAt = [T.b0, T.b1, T.b2, T.b3, T.b4, T.op];
export default {
  ...scene({
    nm: 'Setup · Start in service mode', w: W, h: H, op: T.op, layers,
    markers: beatsAt.slice(0, -1).map((tm, i) => ({ tm, cm: `beat-${i}`, dr: beatsAt[i + 1] - tm })),
  }),
  assets,
  slots: { bgColor: { p: { a: 0, k: rgb(BG) } } },
};
