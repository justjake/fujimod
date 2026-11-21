import PHOTO from '../../../engine/assets/default-scene.png';
import { Artwork, Monogram, filmArtwork, stockArtwork } from '../../../design/artwork';
import { Button, Scroller, Section } from '../../../design/column';
import { AnchoredFlyOut, PickerList, flyOutListHeight, pickerStep } from '../../../design/picker';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Chevron, Choice, Dial, Switch, signed } from '../../../design/controls';
import { Fit, KeyHint, type CameraKey } from '../../../design/screen';
import { ACCENT, COLUMN, DETAIL, GLASS, HEADING, SELECTED, SMALL, TEXT, TRACK } from '../../../design/materials';
import '../../../design/design.css';

/* FILM EDITOR redesign mock: a tool rail down the left edge beside one tall
   scroller holding every tool, all open, over the picture. The rail scrolls
   to a tool and follows the scroller. The picture is developed the way the
   film stage develops it: a 3x3 matrix on linear RGB (COLOR MIX), one shared
   tone curve of ten knots joined by cubic Bezier segments, red and blue
   entering that curve at their own exposure, then a 2x2 on CbCr (COLOR) and
   a chroma gain chosen by output brightness in five bands. */

/* ---- Film model ----------------------------------------------------------------- */

/** Curve knots, on the curve's 12-bit linear input. */
const XS = [0, 64, 128, 256, 512, 1024, 1536, 2048, 3072, 4096];
/** Input position on the stops axis, 0..1: knot 1 is -6 EV, the white knot 0 EV. */
function axis(x: number): number {
  if (x < 64) return Math.max(0, x / 64) / 7;
  return (Math.log2(x / 4096) + 7) / 7;
}
const US = XS.map(axis);
/** Where an output value sits on the graph: stops on both axes, so a neutral curve is a diagonal. */
const disp = (y: number) => axis(decode(Math.max(0, y)) * 4096);
function fromDisp(d: number): number {
  const c = Math.min(1, Math.max(0, d));
  const x = c < 1 / 7 ? c * 7 * 64 : 4096 * 2 ** (7 * c - 7);
  return encode(x / 4096);
}
const ZONES = ['BLACK', 'BLACKS', 'SHADOWS', 'SHADOWS', 'MIDTONES', 'MIDTONES', 'HIGHLIGHTS', 'HIGHLIGHTS', 'WHITES', 'WHITE'];
const KNOT_EV = ['', '−6 EV', '−5 EV', '−4 EV', '−3 EV', '−2 EV', '−1.6 EV', '−1 EV', '−0.4 EV', '0 EV'];

const encode = (l: number) => (l <= 0.0031308 ? 12.92 * l : 1.055 * l ** (1 / 2.4) - 0.055);
const decode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

interface Primary {
  hue: number;
  sat: number;
}

interface Film {
  base: number;
  knots: number[];
  /** Bend added to each knot's slope, in output per axis. */
  tilt: number[];
  saturation: number;
  hue: number;
  warmth: number;
  primaries: Primary[];
  /** COLOR MIX's 3x3 typed in on its NUMBERS page, row by row; wins over primaries. */
  matrix?: Matrix;
  /** COLOR's 2x2 on (Cb, Cr) typed in on its NUMBERS page; wins over saturation, hue and warmth. */
  chroma?: number[];
  edges: number[];
  bands: number[];
  /** Red and blue entering the curve, in tenths of a stop. */
  red: number;
  blue: number;
}

interface Base {
  name: string;
  shape: number[];
  saturation: number;
  hue?: number;
  warmth?: number;
  bands?: number[];
}

const BASES: Base[] = [
  { name: 'PROVIA', shape: [0, -0.01, -0.015, -0.012, 0, 0.012, 0.016, 0.012, 0.005, 0], saturation: 0 },
  { name: 'Velvia', shape: [0, -0.02, -0.035, -0.03, -0.005, 0.02, 0.028, 0.02, 0.008, 0], saturation: 30, bands: [20, 10, 0, -10, -30] },
  { name: 'ASTIA', shape: [0, -0.005, -0.01, -0.006, 0, 0.006, 0.008, 0.004, 0, 0], saturation: 10 },
  { name: 'CLASSIC CHR.', shape: [0, -0.02, -0.04, -0.035, -0.01, 0.005, 0.0, -0.008, -0.012, 0], saturation: -30, warmth: -15, bands: [-10, 0, 0, -10, -30] },
  { name: 'PRO Neg. Std', shape: [0, 0.01, 0.01, 0.006, 0, -0.004, -0.008, -0.008, -0.004, 0], saturation: -10 },
  { name: 'ETERNA', shape: [0.03, 0.04, 0.035, 0.02, 0.005, -0.01, -0.02, -0.025, -0.02, 0], saturation: -35, bands: [-20, -10, 0, -10, -30] },
  { name: 'CLASSIC Neg.', shape: [0, -0.03, -0.05, -0.04, -0.01, 0.02, 0.025, 0.01, -0.01, 0], saturation: -15, hue: 4, warmth: 10 },
];

const EDGES = [64, 120, 200, 230];

export function baseFilm(base: number): Film {
  const b = BASES[base];
  return {
    base,
    knots: XS.map((x, i) => Math.min(1, Math.max(0, encode(x / 4096) + b.shape[i]))),
    tilt: XS.map(() => 0),
    saturation: b.saturation,
    hue: b.hue ?? 0,
    warmth: b.warmth ?? 0,
    primaries: [0, 1, 2].map(() => ({ hue: 0, sat: 0 })),
    edges: [...EDGES],
    bands: [...(b.bands ?? [0, 0, 0, 0, 0])],
    red: 0,
    blue: 0,
  };
}

/** Each knot's slope (output per axis unit), smooth through its neighbors, plus its bend. */
function slopes(f: Film): number[] {
  return f.knots.map((y, i) => {
    const a = Math.max(0, i - 1);
    const b = Math.min(9, i + 1);
    return (f.knots[b] - f.knots[a]) / (US[b] - US[a]) + f.tilt[i];
  });
}

/** Segment i's two Bezier handles as output values: [leaving knot i, entering knot i+1]. */
function handles(f: Film): [number, number][] {
  const m = slopes(f);
  return f.knots.slice(0, 9).map((y, i) => {
    const du = US[i + 1] - US[i];
    return [y + (m[i] * du) / 3, f.knots[i + 1] - (m[i + 1] * du) / 3];
  });
}

function curveAt(f: Film, hs: [number, number][], u: number): number {
  let i = 0;
  while (i < 8 && u > US[i + 1]) i++;
  const t = Math.min(1, Math.max(0, (u - US[i]) / (US[i + 1] - US[i])));
  const [p1, p2] = hs[i];
  const s = 1 - t;
  return s * s * s * f.knots[i] + 3 * s * s * t * p1 + 3 * s * t * t * p2 + t * t * t * f.knots[i + 1];
}

type Matrix = number[];

/** Hue and saturation of each primary, solved into a matrix that keeps white. */
function primariesMatrix(ps: Primary[]): Matrix {
  const g = 1 / 3;
  const axisN = 1 / Math.sqrt(3);
  const columns = ps.map((p, c) => {
    const chroma = [0, 1, 2].map((k) => (k === c ? 1 : 0) - g);
    const a = (p.hue * Math.PI) / 180;
    const cos = Math.cos(a);
    const sin = Math.sin(a);
    const cross = [
      axisN * (chroma[2] - chroma[1]),
      axisN * (chroma[0] - chroma[2]),
      axisN * (chroma[1] - chroma[0]),
    ];
    const scale = 1 + p.sat / 100;
    return chroma.map((v, k) => (v * cos + cross[k] * sin) * scale);
  });
  const drift = [0, 1, 2].map((k) => columns.reduce((sum, col) => sum + col[k], 0) / 3);
  const m: Matrix = [];
  for (let row = 0; row < 3; row++)
    for (let col = 0; col < 3; col++) m.push(g + columns[col][row] - drift[row]);
  return m;
}

const mixMatrix = (f: Film): Matrix => f.matrix ?? primariesMatrix(f.primaries);

/** The hue and saturation per primary nearest a typed-in matrix, so the scope and sliders follow the numbers. */
function fitPrimaries(m: Matrix): Primary[] {
  const n = 1 / Math.sqrt(3);
  return [0, 1, 2].map((c) => {
    const col = [m[c], m[3 + c], m[6 + c]];
    const mean = (col[0] + col[1] + col[2]) / 3;
    const v = col.map((x) => x - mean);
    const h = [0, 1, 2].map((k) => (k === c ? 1 : 0) - 1 / 3);
    const cross = [h[1] * v[2] - h[2] * v[1], h[2] * v[0] - h[0] * v[2], h[0] * v[1] - h[1] * v[0]];
    const hue = (Math.atan2(n * (cross[0] + cross[1] + cross[2]), h[0] * v[0] + h[1] * v[1] + h[2] * v[2]) * 180) / Math.PI;
    const sat = (Math.hypot(v[0], v[1], v[2]) / Math.hypot(h[0], h[1], h[2]) - 1) * 100;
    return { hue: Math.round(hue), sat: Math.round(Math.max(-100, Math.min(100, sat))) };
  });
}

/** COLOR's 2x2 on (Cb, Cr), row by row: hue turns, saturation scales, warm/cool stretches along the orange-blue axis. */
function chromaMatrix(f: Film): number[] {
  if (f.chroma) return f.chroma;
  const s = 1 + f.saturation / 100;
  const a = (f.hue * Math.PI) / 180;
  const dx = -0.6;
  const dy = 0.8;
  const w = f.warmth / 100;
  const k00 = 1 + w * dx * dx;
  const k01 = w * dx * dy;
  const k11 = 1 + w * dy * dy;
  const c = Math.cos(a) * s;
  const sn = Math.sin(a) * s;
  return [c * k00 - sn * k01, c * k01 - sn * k11, sn * k00 + c * k01, sn * k01 + c * k11];
}

/* ---- Developing the picture ----------------------------------------------------- */

const W = 720;
const H = 480;

interface Source {
  pixels: Uint8ClampedArray;
  /** Input brightness on the stops axis, 64 bins. */
  histogram: number[];
}

const sources = new Map<number, Promise<Source>>();

/** RAW files on the SD card, standing in as crops of the default scene. */
const RAWS = [
  { name: 'DSCF0412.RAF', zoom: 1.8, x: 0.62, y: 0.42, info: 'f/2.8 · 1/500 · ISO 200' },
  { name: 'DSCF0413.RAF', zoom: 2.2, x: 0.18, y: 0.5, info: 'f/4 · 1/250 · ISO 400' },
  { name: 'DSCF0415.RAF', zoom: 1.5, x: 0.4, y: 0.2, info: 'f/5.6 · 1/1000 · ISO 200' },
  { name: 'DSCF0418.RAF', zoom: 2.6, x: 0.82, y: 0.62, info: 'f/2.8 · 1/60 · ISO 1600' },
  { name: 'DSCF0421.RAF', zoom: 1.2, x: 0.5, y: 0.5, info: 'f/8 · 1/320 · ISO 200' },
  { name: 'DSCF0422.RAF', zoom: 3, x: 0.3, y: 0.7, info: 'f/2.8 · 1/125 · ISO 800' },
  { name: 'DSCF0427.RAF', zoom: 2, x: 0.7, y: 0.3, info: 'f/4 · 1/800 · ISO 200' },
  { name: 'DSCF0430.RAF', zoom: 1.6, x: 0.1, y: 0.4, info: 'f/2.8 · 1/30 · ISO 3200' },
];

/** Crop 0 is live view; crop n is RAWS[n - 1]. */
function source(crop = 0): Promise<Source> {
  const cached = sources.get(crop);
  if (cached) return cached;
  const promise = new Promise<Source>((resolve) => {
    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement('canvas');
      canvas.width = W;
      canvas.height = H;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('No 2D context');
      const raw = RAWS[crop - 1];
      const scale = Math.max(W / image.width, H / image.height) * (raw?.zoom ?? 1);
      const w = image.width * scale;
      const h = image.height * scale;
      const x = raw ? Math.min(0, Math.max(W - w, W / 2 - raw.x * w)) : (W - w) / 2;
      const y = raw ? Math.min(0, Math.max(H - h, H / 2 - raw.y * h)) : (H - h) / 2;
      context.drawImage(image, x, y, w, h);
      const pixels = context.getImageData(0, 0, W, H).data;
      const histogram = Array.from({ length: 64 }, () => 0);
      for (let p = 0; p < pixels.length; p += 4) {
        const l = 0.2126 * DECODE[pixels[p]] + 0.7152 * DECODE[pixels[p + 1]] + 0.0722 * DECODE[pixels[p + 2]];
        histogram[Math.min(63, Math.floor(axis(l * 4096) * 64))]++;
      }
      const peak = Math.max(...histogram);
      resolve({ pixels, histogram: histogram.map((n) => Math.sqrt(n / peak)) });
    };
    image.src = PHOTO;
  });
  sources.set(crop, promise);
  return promise;
}

const DECODE = Array.from({ length: 256 }, (_, v) => decode(v / 255));

/** Each output brightness's chroma gain from the five bands. */
function bandGains(f: Film): Float32Array {
  const band = new Float32Array(256);
  const centers = [0, ...f.edges, 255];
  for (let y = 0; y < 256; y++) {
    let i = 0;
    while (i < 4 && y > f.edges[i]) i++;
    const mid = (centers[i] + centers[i + 1]) / 2;
    const next = y < mid ? Math.max(0, i - 1) : Math.min(4, i + 1);
    const nextMid = (centers[next] + centers[next + 1]) / 2;
    const t = next === i ? 0 : Math.min(0.5, Math.abs(y - mid) / Math.max(1, Math.abs(nextMid - mid)));
    band[y] = 1 + ((1 - t) * f.bands[i] + t * f.bands[next]) / 100;
  }
  return band;
}

/** The recipe's other settings, for CUSTOM SETTING on: a little more
 * contrast (HIGHLIGHT +1, SHADOW -1) and warmer white balance. */
function customSettings(v: number, channel: number): number {
  const contrast = 0.5 + (v - 0.5) * 1.1;
  return contrast * [1.04, 1, 0.94][channel];
}

function develop(f: Film, src: Source, out: ImageData, custom: boolean) {
  const hs = handles(f);
  const lut = new Float32Array(4097);
  for (let x = 0; x <= 4096; x++) lut[x] = curveAt(f, hs, axis(x));
  const m = mixMatrix(f);
  const gr = 2 ** (f.red / 10);
  const gb = 2 ** (f.blue / 10);
  const [a00, a01, a10, a11] = chromaMatrix(f);
  const band = bandGains(f);
  const px = src.pixels;
  const o = out.data;
  for (let p = 0; p < px.length; p += 4) {
    const r = DECODE[px[p]];
    const g = DECODE[px[p + 1]];
    const b = DECODE[px[p + 2]];
    const R = lut[Math.min(4096, Math.max(0, Math.round((m[0] * r + m[1] * g + m[2] * b) * gr * 4096)))];
    const G = lut[Math.min(4096, Math.max(0, Math.round((m[3] * r + m[4] * g + m[5] * b) * 4096)))];
    const B = lut[Math.min(4096, Math.max(0, Math.round((m[6] * r + m[7] * g + m[8] * b) * gb * 4096)))];
    const Y = 0.299 * R + 0.587 * G + 0.114 * B;
    const cb = (B - Y) * 0.564;
    const cr = (R - Y) * 0.713;
    const gain = band[Math.min(255, Math.max(0, Math.round(Y * 255)))];
    const cb2 = (a00 * cb + a01 * cr) * gain;
    const cr2 = (a10 * cb + a11 * cr) * gain;
    const R2 = Y + 1.402 * cr2;
    const B2 = Y + 1.773 * cb2;
    const G2 = (Y - 0.299 * R2 - 0.114 * B2) / 0.587;
    o[p] = (custom ? customSettings(R2, 0) : R2) * 255;
    o[p + 1] = (custom ? customSettings(G2, 1) : G2) * 255;
    o[p + 2] = (custom ? customSettings(B2, 2) : B2) * 255;
    o[p + 3] = 255;
  }
}

function Canvas(props: { film: Film; custom: boolean; crop: number; style?: JSX.CSSProperties }) {
  let canvas: HTMLCanvasElement | undefined;
  const [src, setSrc] = createSignal<{ crop: number; source: Source }>();
  let frame = 0;
  createEffect(
    () => props.crop,
    (crop) => {
      void source(crop).then((s) => setSrc({ crop, source: s }));
    },
  );
  createEffect(
    () => [props.film, props.custom, src()] as const,
    ([film, custom, s]) => {
      if (!s || !canvas) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const context = canvas?.getContext('2d');
        if (!context) return;
        const out = context.createImageData(W, H);
        develop(film, s.source, out, custom);
        context.putImageData(out, 0, 0);
      });
    },
  );
  return <canvas ref={(element) => (canvas = element)} width={W} height={H} class="absolute inset-0" style={props.style} />;
}

/** The picture, developed with the film; in compare, B's development shows
 * right of the divider. */
export function Picture(props: { film: Film; custom: boolean; crop: number; compare?: { film: Film; split: number } }) {
  return (
    <>
      <Canvas film={props.film} custom={props.custom} crop={props.crop} />
      <Show when={props.compare}>
        {(c) => (
          <Canvas film={c().film} custom={props.custom} crop={props.crop} style={{ 'clip-path': `inset(0 0 0 ${c().split}px)` }} />
        )}
      </Show>
    </>
  );
}

/* ---- Look ------------------------------------------------------------------------ */

const BAND_NAMES = ['BLACKS', 'SHADOWS', 'MIDTONES', 'HIGHLIGHTS', 'WHITES'];



/* ---- Controls --------------------------------------------------------------------- */


/** A labeled row: name on the left, the control in its track on the right. */

function curvePath(f: Film, w: number, h: number): string {
  const hs = handles(f);
  const n = 140;
  let d = '';
  for (let k = 0; k <= n; k++) {
    const u = k / n;
    d += `${k ? ' L' : 'M'}${(u * w).toFixed(1)},${((1 - disp(curveAt(f, hs, u))) * h).toFixed(1)}`;
  }
  return d;
}

interface CurveProps {
  film: Film;
  opened: Film;
  histogram: number[];
  width: number;
  height: number;
  focus: number;
  bend: boolean;
  /** A tone picked in the picture, on the stops axis. */
  picked?: number;
  set: (film: Film) => void;
  focusKnot: (knot: number) => void;
  /** Lines and knots only, for drawing over the picture. */
  bare?: boolean;
}

/** The tone curve: knots on fixed brightness columns, joined by Bezier
 * segments. A finger drags a knot up and down; in BEND, the knot's two
 * handles tilt the curve through it. */
function CurveGraph(props: CurveProps) {
  const pad = 14;
  const w = () => props.width - 2 * pad;
  const h = () => props.height - 2 * pad;
  const X = (u: number) => pad + u * w();
  const Y = (v: number) => pad + (1 - disp(v)) * h();
  const Yh = (d: number) => pad + (1 - d) * h();
  const changed = (i: number) => Math.abs(props.film.knots[i] - props.opened.knots[i]) > 1e-4 || props.film.tilt[i] !== 0;
  let drag: { kind: 'knot' | 'handle'; knot: number; y: number; start: number } | undefined;
  const local = (e: PointerEvent, element: Element) => {
    const box = element.getBoundingClientRect();
    return { x: ((e.clientX - box.left) / box.width) * props.width, y: ((e.clientY - box.top) / box.height) * props.height };
  };
  const histogramPath = () => {
    const bins = props.histogram;
    let d = `M${X(0)},${Yh(0)}`;
    bins.forEach((v, i) => {
      d += ` L${X((i + 0.5) / bins.length)},${Yh(v * 0.85)}`;
    });
    return `${d} L${X(1)},${Yh(0)} Z`;
  };
  const handlePoints = () => {
    const i = props.focus;
    const m = slopes(props.film)[i];
    const left = i > 0 ? US[i] - (US[i] - US[i - 1]) / 3 : undefined;
    const right = i < 9 ? US[i] + (US[i + 1] - US[i]) / 3 : undefined;
    const at = (u: number) => ({ u, v: props.film.knots[i] + m * (u - US[i]) });
    return [left, right].filter((u): u is number => u !== undefined).map(at);
  };
  return (
    <svg
      width={props.width}
      height={props.height}
      class="block touch-none"
      onPointerDown={(e) => {
        const element = e.currentTarget;
        element.setPointerCapture(e.pointerId);
        const p = local(e, element);
        if (props.bend) {
          const near = handlePoints().find((q) => Math.hypot(X(q.u) - p.x, Y(q.v) - p.y) < 22);
          if (near) {
            drag = { kind: 'handle', knot: props.focus, y: p.y, start: props.film.tilt[props.focus] };
            return;
          }
        }
        let knot = 0;
        for (let i = 1; i < 10; i++) if (Math.abs(X(US[i]) - p.x) < Math.abs(X(US[knot]) - p.x)) knot = i;
        props.focusKnot(knot);
        drag = { kind: 'knot', knot, y: p.y, start: props.film.knots[knot] };
      }}
      onPointerMove={(e) => {
        if (!drag) return;
        const p = local(e, e.currentTarget);
        const dv = (drag.y - p.y) / h();
        if (drag.kind === 'knot') {
          const knots = [...props.film.knots];
          knots[drag.knot] = fromDisp(disp(drag.start) + dv);
          props.set({ ...props.film, knots });
        } else {
          const tilt = [...props.film.tilt];
          const du = (US[Math.min(9, drag.knot + 1)] - US[Math.max(0, drag.knot - 1)]) / 6;
          tilt[drag.knot] = drag.start + dv / du;
          props.set({ ...props.film, tilt });
        }
      }}
      onPointerUp={() => (drag = undefined)}
    >
      <Show when={!props.bare}>
        <path d={histogramPath()} fill="rgb(255 255 255 / 0.13)" />
      </Show>
      <For each={US}>
        {(u, i) => (
          <line
            x1={X(u)}
            x2={X(u)}
            y1={pad}
            y2={pad + h()}
            stroke={i() === props.focus ? 'rgb(255 255 255 / 0.55)' : 'rgb(255 255 255 / 0.14)'}
            stroke-width={i() === props.focus ? 2 : 1}
          />
        )}
      </For>
      <path d={curvePath(props.opened, w(), h())} transform={`translate(${pad},${pad})`} fill="none" stroke="rgb(255 255 255 / 0.35)" stroke-width="1.5" stroke-dasharray="4 4" />
      <path d={curvePath(props.film, w(), h())} transform={`translate(${pad},${pad})`} fill="none" stroke="#fff" stroke-width="3" />
      <Show when={props.picked !== undefined}>
        <line x1={X(props.picked ?? 0)} x2={X(props.picked ?? 0)} y1={pad} y2={pad + h()} stroke="#ffd60a" stroke-width="2" />
      </Show>
      <Show when={props.bend}>
        <For each={handlePoints()}>
          {(q) => (
            <>
              <line x1={X(US[props.focus])} y1={Y(props.film.knots[props.focus])} x2={X(q.u)} y2={Y(q.v)} stroke="#fff" stroke-width="2" />
              <circle cx={X(q.u)} cy={Y(q.v)} r="7" fill="#fff" />
            </>
          )}
        </For>
      </Show>
      <For each={props.film.knots}>
        {(v, i) => (
          <circle
            cx={X(US[i()])}
            cy={Y(v)}
            r={i() === props.focus ? 10 : 6.5}
            fill={changed(i()) ? ACCENT : i() === props.focus ? '#fff' : '#1c1c1e'}
            stroke="#fff"
            stroke-width={i() === props.focus ? 3 : 2}
          />
        )}
      </For>
    </svg>
  );
}

function knotReadout(f: Film, knot: number, bend: boolean): string {
  const out = Math.round(f.knots[knot] * 100);
  const where = knot === 0 ? 'BLACK POINT' : knot === 9 ? 'WHITE POINT' : `${ZONES[knot]} ${KNOT_EV[knot]}`;
  return bend ? `${where} · BEND ${signed(Math.round(f.tilt[knot] * 10))}` : `${where} · ${out}%`;
}

/* ---- Tools ----------------------------------------------------------------------- */

type Tool = 'curve' | 'color' | 'mix' | 'tone' | 'channels';
const TOOLS: Tool[] = ['curve', 'color', 'mix', 'tone', 'channels'];
/** The scroller's sections: the RAW file while one is shown, the custom
 * setting the film is seen through, then the film's tools. */
type Section = 'film' | 'raw' | 'custom' | Tool;
/** The stops above the tools: the film's name and files, the RAW file and the
 * custom setting. */
type Meta = 'icon' | 'name' | 'save' | 'saveas' | 'open' | 'strip' | 'reflect' | RawOption | 'create' | 'custom';

/** How a RAW is developed. These belong to the conversion, not to the custom setting or the film: they are never saved into either. */
const RAW_OPTIONS = [
  { key: 'push', name: 'PUSH/PULL PROCESSING', values: ['−1 EV', '−2/3 EV', '−1/3 EV', '0', '+1/3 EV', '+2/3 EV', '+1 EV', '+1 1/3 EV', '+1 2/3 EV', '+2 EV', '+2 1/3 EV', '+2 2/3 EV', '+3 EV'], start: 3 },
  { key: 'space', name: 'COLOR SPACE', values: ['sRGB', 'Adobe RGB'], start: 0 },
  { key: 'filetype', name: 'FILE TYPE', values: ['JPEG', 'HEIF', 'TIFF 8BIT', 'TIFF 16BIT'], start: 0 },
  { key: 'size', name: 'IMAGE SIZE', values: ['L 3:2', 'M 3:2', 'S 3:2'], start: 0 },
  { key: 'quality', name: 'IMAGE QUALITY', values: ['FINE', 'NORMAL'], start: 0 },
  { key: 'lmo', name: 'LENS MODULATION OPTIMIZER', values: ['ON', 'OFF'], start: 0 },
] as const;
type RawOption = (typeof RAW_OPTIONS)[number]['key'];
const RAW_DEFAULT: number[] = RAW_OPTIONS.map((o) => o.start);
const META_SECTION: Record<Meta, Section> = {
  icon: 'film',
  name: 'film',
  save: 'film',
  saveas: 'film',
  open: 'film',
  strip: 'raw',
  reflect: 'raw',
  push: 'raw',
  space: 'raw',
  filetype: 'raw',
  size: 'raw',
  quality: 'raw',
  lmo: 'raw',
  create: 'raw',
  custom: 'custom',
};
const TOOL_NAMES: Record<Tool, string> = {
  curve: 'TONE CURVE',
  color: 'COLOR',
  mix: 'COLOR MIX',
  tone: 'SATURATION BY TONE',
  channels: 'CHANNELS',
};

const GROUPS: Record<Tool, (f: Film) => unknown> = {
  curve: (f) => [f.knots, f.tilt],
  color: (f) => [f.saturation, f.hue, f.warmth, f.chroma],
  mix: (f) => [f.primaries, f.matrix],
  tone: (f) => [f.bands, f.edges],
  channels: (f) => [f.red, f.blue],
};

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** The film a tool's settings came from, or none once edited. */
function groupSource(f: Film, tool: Tool): number | undefined {
  const value = GROUPS[tool](f);
  if (same(GROUPS[tool](baseFilm(f.base)), value)) return f.base;
  const k = BASES.findIndex((_, i) => same(GROUPS[tool](baseFilm(i)), value));
  return k < 0 ? undefined : k;
}

function copyGroup(f: Film, tool: Tool, k: number): Film {
  const b = baseFilm(k);
  switch (tool) {
    case 'curve':
      return { ...f, knots: b.knots, tilt: b.tilt };
    case 'color':
      return { ...f, saturation: b.saturation, hue: b.hue, warmth: b.warmth, chroma: undefined };
    case 'mix':
      return { ...f, primaries: b.primaries, matrix: undefined };
    case 'tone':
      return { ...f, bands: b.bands, edges: b.edges };
    case 'channels':
      return { ...f, red: b.red, blue: b.blue };
  }
}

interface Scale {
  name: string;
  get: (f: Film) => number;
  put: (f: Film, v: number) => Film;
  min: number;
  max: number;
  step: number;
  track: string;
}

const gradient = (...stops: string[]) => `linear-gradient(to right, ${stops.join(', ')})`;
const GRAY = '#8e8e93';

const COLOR_SCALES: Scale[] = [
  { name: 'SATURATION', get: (f) => f.saturation, put: (f, v) => ({ ...f, saturation: v, chroma: undefined }), min: -100, max: 100, step: 5, track: gradient(GRAY, '#ff9500') },
  { name: 'HUE', get: (f) => f.hue, put: (f, v) => ({ ...f, hue: v, chroma: undefined }), min: -180, max: 180, step: 1, track: gradient('#00c7be', '#0a84ff', '#bf5af2', '#ff375f', '#ff9f0a', '#ffd60a', '#30d158', '#00c7be') },
  { name: 'WARM/COOL', get: (f) => f.warmth, put: (f, v) => ({ ...f, warmth: v, chroma: undefined }), min: -100, max: 100, step: 5, track: gradient('#0a84ff', GRAY, '#ff9f0a') },
];

/** COLOR MIX: how reds, greens and blues render. HUE turns them toward
 * their neighbors (reds toward pink or orange); SATURATION makes them
 * stronger or weaker. Grays stay gray. */
const MIX = [
  { name: 'REDS', color: '#ff3b30', linear: [0.75, 0.06, 0.04], hue: gradient('#ff2d78', '#ff3b30', '#ff9500') },
  { name: 'GREENS', color: '#34c759', linear: [0.08, 0.5, 0.1], hue: gradient('#b5d334', '#34c759', '#00c7be') },
  { name: 'BLUES', color: '#0a64ff', linear: [0.04, 0.1, 0.7], hue: gradient('#32ade6', '#0a64ff', '#a35bff') },
];
const mixScales = (p: number): Scale[] => [
  {
    name: 'HUE',
    get: (f) => f.primaries[p].hue,
    put: (f, v) => ({ ...f, matrix: undefined, primaries: f.primaries.map((q, i) => (i === p ? { ...q, hue: v } : q)) }),
    min: -180,
    max: 180,
    step: 2,
    track: MIX[p].hue,
  },
  {
    name: 'SATURATION',
    get: (f) => f.primaries[p].sat,
    put: (f, v) => ({ ...f, matrix: undefined, primaries: f.primaries.map((q, i) => (i === p ? { ...q, sat: v } : q)) }),
    min: -100,
    max: 100,
    step: 5,
    track: gradient(GRAY, MIX[p].color),
  },
];

const CHANNEL_SCALES: Scale[] = [
  { name: 'RED CURVE', get: (f) => f.red, put: (f, v) => ({ ...f, red: v }), min: -5, max: 5, step: 1, track: gradient('#00c7be', GRAY, '#ff3b30') },
  { name: 'BLUE CURVE', get: (f) => f.blue, put: (f, v) => ({ ...f, blue: v }), min: -5, max: 5, step: 1, track: gradient('#ffd60a', GRAY, '#0a84ff') },
];

/** A focus stop. COLOR MIX's two scales edit the primary its chips chose (pick: 0 hue, 1 saturation). */
type Control =
  | { tool: Tool; kind: 'curve' }
  | { tool: Tool; kind: 'tone' }
  | { tool: Tool; kind: 'chips' }
  | { tool: 'mix' | 'color'; kind: 'numbers' }
  | { tool: Tool; kind: 'scale'; scale: Scale; pick?: number };

const CONTROLS: Control[] = [
  { tool: 'curve', kind: 'curve' },
  ...COLOR_SCALES.map((scale): Control => ({ tool: 'color', kind: 'scale', scale })),
  { tool: 'color', kind: 'numbers' },
  { tool: 'mix', kind: 'chips' },
  ...mixScales(0).map((scale, pick): Control => ({ tool: 'mix', kind: 'scale', scale, pick })),
  { tool: 'mix', kind: 'numbers' },
  { tool: 'tone', kind: 'tone' },
  ...CHANNEL_SCALES.map((scale): Control => ({ tool: 'channels', kind: 'scale', scale })),
];

/* ---- Saturation by tone ----------------------------------------------------------- */

function hsv(h: number, s: number, v: number): [number, number, number] {
  const f = (n: number) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return [f(5), f(3), f(1)];
}

const BAND_CENTERS = (f: Film) => {
  const b = [0, ...f.edges, 255];
  return [0, 1, 2, 3, 4].map((i) => (b[i] + b[i + 1]) / 2);
};

/** Saturation by tone: one column per brightness range, darkest at the
 * left. Each column is drawn at its brightness; its knob is a color at that
 * brightness and the saturation the column gives it. Up is more color. */
function ToneGraph(props: { film: Film; opened: Film; width: number; height: number; band: number; inside: boolean; set: (film: Film) => void; focusBand: (band: number) => void }) {
  const gap = 6;
  const cw = () => (props.width - 4 * gap) / 5;
  const Y = (pct: number) => props.height / 2 - (pct / 100) * (props.height / 2 - 16);
  const level = (i: number) => 0.12 + (BAND_CENTERS(props.film)[i] / 255) * 0.83;
  const sample = (i: number) => {
    const gain = Math.max(0, 1 + props.film.bands[i] / 100);
    const [r, g, b] = hsv(24, Math.min(1, 0.7 * gain), level(i));
    return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
  };
  let drag: { band: number; y: number; start: number } | undefined;
  const local = (e: PointerEvent, element: Element) => {
    const box = element.getBoundingClientRect();
    return { x: ((e.clientX - box.left) / box.width) * props.width, y: ((e.clientY - box.top) / box.height) * props.height };
  };
  return (
    <svg
      width={props.width}
      height={props.height}
      class="block touch-none"
      onPointerDown={(e) => {
        const element = e.currentTarget;
        element.setPointerCapture(e.pointerId);
        const p = local(e, element);
        const band = Math.max(0, Math.min(4, Math.floor(p.x / (cw() + gap))));
        props.focusBand(band);
        drag = { band, y: p.y, start: props.film.bands[band] };
      }}
      onPointerMove={(e) => {
        if (!drag) return;
        const p = local(e, e.currentTarget);
        const bands = [...props.film.bands];
        const pct = drag.start + ((drag.y - p.y) / (props.height / 2 - 16)) * 100;
        bands[drag.band] = Math.max(-100, Math.min(100, Math.round(pct / 5) * 5));
        props.set({ ...props.film, bands });
      }}
      onPointerUp={() => (drag = undefined)}
    >
      <For each={[0, 1, 2, 3, 4]}>
        {(i) => {
          const x = () => i * (cw() + gap);
          const v = () => Math.round(level(i) * 255 * 0.8);
          const pct = () => props.film.bands[i];
          const chosen = () => props.inside && props.band === i;
          return (
            <>
              <rect x={x()} y="0" width={cw()} height={props.height} rx="8" fill={`rgb(${v()} ${v()} ${v()})`} />
              <line x1={x() + 8} x2={x() + cw() - 8} y1={Y(0)} y2={Y(0)} stroke="rgb(255 255 255 / 0.5)" stroke-dasharray="4 4" />
              <line x1={x() + cw() / 2} x2={x() + cw() / 2} y1={Y(0)} y2={Y(pct())} stroke={pct() !== props.opened.bands[i] ? ACCENT : '#fff'} stroke-width="4" stroke-linecap="round" />
              <circle cx={x() + cw() / 2} cy={Y(pct())} r={chosen() ? 15 : 12} fill={sample(i)} stroke={chosen() ? '#fff' : 'rgb(255 255 255 / 0.7)'} stroke-width={chosen() ? 4 : 2} />
            </>
          );
        }}
      </For>
    </svg>
  );
}

/* ---- Editor ----------------------------------------------------------------------- */

export interface State {
  film: Film;
  opened: Film;
  focus: number;
  knot: number;
  /** The dial last bent the chosen knot: its handles show. */
  bend: boolean;
  band: number;
  /** OK went inside the focused tone curve or saturation by tone. */
  inside: boolean;
  /** The focus is on the heading of the focused control's tool. */
  header: boolean;
  /** CUSTOM SETTING: the recipe's other settings applied on top of the film. */
  custom: boolean;
  compare: { b: number | 'opened'; split: number } | null;
  /** 0: live view; n: RAWS[n - 1]. */
  crop: number;
  picker: PickerState | null;
  /** Where the D-pad is: the RAW strip, the tool rail or the tool rows. */
  area: Meta | 'rail' | 'rows';
  /** On the rail, the film, RAW or CUSTOM SETTING section instead of a tool. */
  railOn: 'film' | 'raw' | 'custom' | null;
  /** The film's name on the SD card, and the film it was made from. */
  name: string;
  from: string | null;
  /** The film's icon, a FILM ICONS slug. */
  icon: string;
  /** Typing the film's name. */
  keyboard: Typing | null;
  /** The tone curve over the whole picture. */
  full: boolean;
  strip: StripStyle;
  /** The RAW last developed with OK. */
  created: number;
  /** The RAW conversion's options, an index into each of RAW_OPTIONS' values. */
  raw: number[];
  /** The primary COLOR MIX's scales edit: 0 reds, 1 greens, 2 blues. */
  primary: number;
  /** A matrix shown as numbers over the column, with the focused cell. */
  numbers: { tool: 'mix' | 'color'; cell: number } | null;
  /** Review states open scrolled to a tool. */
  scrollTo?: Section;
}

/** The scale a control edits now: COLOR MIX's follow the chosen primary. */
const scaleOf = (c: Control & { kind: 'scale' }, s: State): Scale => (c.pick === undefined ? c.scale : mixScales(s.primary)[c.pick]);

/** A matrix's cells for its NUMBERS page, row by row. */
const MATRIX_SIZE = { mix: 3, color: 2 } as const;
const cells = (f: Film, tool: 'mix' | 'color'): number[] => (tool === 'mix' ? mixMatrix(f) : chromaMatrix(f));
/** COLOR MIX's diagonal follows its row so white stays white. */
const fixedCell = (tool: 'mix' | 'color', cell: number) => tool === 'mix' && cell % 4 === 0;

function putCell(f: Film, tool: 'mix' | 'color', cell: number, v: number): Film {
  const m = [...cells(f, tool)];
  m[cell] = Math.round(v * 100) / 100;
  if (tool === 'color') {
    const hue = Math.round((Math.atan2(m[2], m[0]) * 180) / Math.PI);
    const saturation = Math.round((Math.hypot(m[0], m[2]) - 1) * 100);
    return { ...f, chroma: m, hue, saturation: Math.max(-100, Math.min(100, saturation)) };
  }
  const row = Math.floor(cell / 3) * 3;
  const diag = row + row / 3;
  m[diag] = Math.round((1 - m.slice(row, row + 3).reduce((sum, x, k) => (row + k === diag ? sum : sum + x), 0)) * 100) / 100;
  return { ...f, matrix: m, primaries: fitPrimaries(m) };
}

function pressNumbers(s: State, n: NonNullable<State['numbers']>, key: CameraKey): State {
  const size = MATRIX_SIZE[n.tool];
  const move = (dr: number, dc: number) => {
    let r = Math.floor(n.cell / size);
    let c = n.cell % size;
    for (let tries = 0; tries < size; tries++) {
      r = Math.max(0, Math.min(size - 1, r + dr));
      c = Math.max(0, Math.min(size - 1, c + dc));
      if (!fixedCell(n.tool, r * size + c)) return { ...s, numbers: { ...n, cell: r * size + c } };
    }
    return s;
  };
  switch (key) {
    case 'left':
      return move(0, -1);
    case 'right':
      return move(0, 1);
    case 'up':
      return move(-1, 0);
    case 'down':
      return move(1, 0);
    case 'dial-':
    case 'dial+':
      return { ...s, film: putCell(s.film, n.tool, n.cell, cells(s.film, n.tool)[n.cell] + (key === 'dial+' ? 0.01 : -0.01)) };
    case 'ok':
    case 'back':
      return { ...s, numbers: null };
  }
  return s;
}

const START = baseFilm(3);

/** An edited film for the review states. */
const EDITED: Film = (() => {
  const f = baseFilm(3);
  const knots = [...f.knots];
  knots[0] += 0.05;
  knots[1] += 0.035;
  knots[3] -= 0.02;
  knots[7] += 0.03;
  const tilt = [...f.tilt];
  tilt[4] = 0.25;
  return {
    ...f,
    knots,
    tilt,
    saturation: -20,
    primaries: [{ hue: 6, sat: -10 }, { hue: -8, sat: -25 }, { hue: 4, sat: 10 }],
    bands: [-20, 0, 10, -10, -40],
    red: 1,
    blue: -1,
  };
})();

type PickerState = { kind: 'icon'; at: number } | { kind: 'copy'; tool: Tool; at: number } | { kind: 'compare'; at: number } | { kind: 'open'; at: number };

/** Films on the SD card, standing in as edits of the built-in ones. */
const CARD_FILMS: { name: string; from: string; icon: string; film: () => Film }[] = [
  { name: 'NEW FILM', from: 'Classic Chrome', icon: 'classic-chrome', film: () => EDITED },
  { name: 'WARM FADE', from: 'CLASSIC Neg.', icon: 'custom', film: () => ({ ...baseFilm(6), saturation: -25, bands: [-30, -10, 0, 0, -20] }) },
  { name: 'NIGHT STREET', from: 'ETERNA', icon: 'eterna', film: () => ({ ...baseFilm(5), red: 1, blue: 1 }) },
  { name: 'SOFT PORTRAIT', from: 'ASTIA', icon: 'astia', film: () => ({ ...baseFilm(2), saturation: -5 }) },
];

/* Our own film artwork (the FILM ICONS packs), by the slug a film file's
   "icon" names. */
const ICONS = ['provia', 'velvia', 'astia', 'classic-chrome', 'pro-neg-hi', 'pro-neg-std', 'eterna', 'bleach-bypass', 'classic-neg', 'nostalgic-neg', 'reala-ace', 'neutral',
  'acros', 'acros-ye', 'acros-r', 'acros-g', 'mono', 'mono-ye', 'mono-r', 'mono-g', 'sepia', 'leica-x', 'custom', 'text:'];
/** Our own letters: 1 to 4, capitals or small capitals (typed small), in the tile. */
const MONOGRAM_LIMIT = 4;
const ICON_COLUMNS = 6;
const ICON_CELL = 60;
const BASE_ICONS = ['provia', 'velvia', 'astia', 'classic-chrome', 'pro-neg-std', 'eterna', 'classic-neg'];

function FilmIcon(props: { slug: string; size: number; color: string }) {
  const text = () => (props.slug.startsWith('text:') ? props.slug.slice(5) : undefined);
  const monogram = () => <Monogram text={text() ?? props.slug.slice(0, 2).toUpperCase()} size={props.size} color={props.color} />;
  return (
    <Artwork
      src={text() === undefined ? filmArtwork(props.slug) : undefined}
      size={props.size}
      color={props.color}
      fallback={monogram()}
    />
  );
}

/** Options are film numbers: the SD card's films, then the built-in ones. */
function pickerOptions(p: PickerState): (number | 'opened')[] {
  if (p.kind === 'icon') return ICONS.map((_, i) => i);
  if (p.kind === 'open') return [...CARD_FILMS.map((_, i) => i), ...BASES.map((_, i) => CARD_FILMS.length + i)];
  return p.kind === 'compare' ? ['opened', ...BASES.map((_, i) => i)] : BASES.map((_, i) => i);
}

function pickerLabel(p: PickerState, k: number | 'opened'): string {
  if (k === 'opened') return 'AS OPENED';
  if (p.kind === 'icon') return ICONS[k];
  if (p.kind !== 'open') return BASES[k].name;
  return k < CARD_FILMS.length ? CARD_FILMS[k].name : BASES[k - CARD_FILMS.length].name;
}

function pickerCurrent(s: State, p: PickerState): number | 'opened' | undefined {
  if (p.kind === 'icon') return s.icon.startsWith('text:') ? ICONS.length - 1 : ICONS.indexOf(s.icon);
  if (p.kind === 'copy') return groupSource(s.film, p.tool);
  if (p.kind === 'compare') return s.compare?.b;
  const card = CARD_FILMS.findIndex((c) => c.name === s.name);
  return card < 0 ? undefined : card;
}

function choose(s: State, p: PickerState, k: number | 'opened'): State {
  if (k === 'opened') return p.kind === 'compare' ? { ...s, picker: null, compare: { b: k, split: s.compare?.split ?? SPLIT } } : s;
  switch (p.kind) {
    case 'icon':
      if (ICONS[k] === 'text:') return { ...s, picker: null, keyboard: typing(s.icon.startsWith('text:') ? s.icon.slice(5) : '', 'icon') };
      return { ...s, picker: null, icon: ICONS[k] };
    case 'copy':
      return { ...s, picker: null, film: copyGroup(s.film, p.tool, k) };
    case 'compare':
      return { ...s, picker: null, compare: { b: k, split: s.compare?.split ?? SPLIT } };
    case 'open': {
      /* An SD card film opens as itself; a built-in one as a new film made from it. */
      if (k < CARD_FILMS.length) {
        const card = CARD_FILMS[k];
        return { ...s, picker: null, name: card.name, from: card.from, icon: card.icon, film: card.film(), opened: card.film() };
      }
      const base = k - CARD_FILMS.length;
      return { ...s, picker: null, name: 'NEW FILM', from: BASES[base].name, icon: BASE_ICONS[base], film: baseFilm(base), opened: baseFilm(base) };
    }
  }
}

/** SAVE AS NEW's suggested name: the film's name numbered on. */
function nextName(name: string): string {
  const match = /^(.*?)(?: (\d+))?$/.exec(name);
  return `${match?.[1] ?? name} ${Number(match?.[2] ?? 1) + 1}`;
}

/** SAVE AS NEW: the edits are written to a new film under the typed name, and editing goes on there; the film opened stays as it was saved. */
function saveAs(s: State, name: string): State {
  return { ...s, name, from: s.name, opened: s.film };
}

/** SAVE: the edits are written over the film opened. */
const save = (s: State): State => ({ ...s, opened: s.film });

function openPicker(s: State, p: { kind: 'icon' } | { kind: 'copy'; tool: Tool } | { kind: 'compare' } | { kind: 'open' }): State {
  const current = pickerCurrent(s, { ...p, at: 0 });
  const at = Math.max(0, pickerOptions({ ...p, at: 0 }).indexOf(current ?? -1));
  return { ...s, picker: { ...p, at } };
}

export function initial(over: Partial<State>): State {
  return { film: EDITED, opened: START, focus: 0, knot: 4, bend: false, band: 1, inside: false, header: false, custom: true, compare: null, crop: 0, picker: null, area: 'rows', railOn: null, name: 'NEW FILM', from: 'Classic Chrome', icon: 'classic-chrome', keyboard: null, full: false, strip: 'frames', created: 0, raw: RAW_DEFAULT, primary: 0, numbers: null, ...over };
}

export const RAIL = 60;
export const SCROLLER = 392;
const CONTENT = SCROLLER - 12;
const PICTURE_LEFT = RAIL + SCROLLER;
const SPLIT = Math.round((PICTURE_LEFT + W) / 2);

function compareFilm(s: State): Film | undefined {
  if (!s.compare) return undefined;
  return s.compare.b === 'opened' ? s.opened : baseFilm(s.compare.b);
}


const firstOf = (tool: Tool) => CONTROLS.findIndex((c) => c.tool === tool);

function railSections(s: State): Section[] {
  return ['film', ...(s.crop ? (['raw'] as Section[]) : []), 'custom', ...TOOLS];
}

/** The stops above the tools, a row at a time; a row of two buttons is one row. */
function metaRows(s: State): Meta[][] {
  return [['icon', 'name'], ['save', 'saveas', 'open'], ...(s.crop ? ([['strip'], ['reflect'], ...RAW_OPTIONS.map((o) => [o.key]), ['create']] as Meta[][]) : []), ['custom']];
}

/** The section the focus is in, or on the rail, the section chosen there. */
function currentSection(s: State): Section {
  if (s.area === 'rail') return s.railOn ?? CONTROLS[s.focus].tool;
  if (s.area === 'rows') return CONTROLS[s.focus].tool;
  return META_SECTION[s.area];
}

function enterSection(s: State, section: Section): State {
  if (section === 'film') return { ...s, area: 'icon', railOn: null };
  if (section === 'raw') return { ...s, area: 'strip', railOn: null };
  if (section === 'custom') return { ...s, area: 'custom', railOn: null };
  return { ...s, area: 'rows', railOn: null, focus: firstOf(section), header: false };
}

/* ---- Name keyboard ------------------------------------------------------------- */

/* Focal UI's text keyboard (TextKeyboard), presented over live view: the
   title, the text with its caret, and the tray of keys under a bar of caret
   keys. Places and widths are in row units, 100 to a row, as on the camera.
   Shift gives capitals; an icon's lowercase letters are its small capitals. */
type KeyKind = 'char' | 'shift' | 'delete' | 'layer' | 'space' | 'return' | 'caret-left' | 'caret-right';
type KeyCap = { kind: KeyKind; row: number; left: number; width: number; label: string };
type Typing = { text: string; cursor: number; focus: number; shift: 'off' | 'once' | 'locked'; digits: boolean; target: TypingTarget };
/** What the keyboard names: the film (rename), a copy (SAVE AS NEW) or the lettered icon. */
type TypingTarget = 'name' | 'saveas' | 'icon';

function keyCaps(t: Typing): KeyCap[] {
  const rows = t.digits ? ['1234567890', "-/:;()&@'", '.,?!+#*'] : ['qwertyuiop', 'asdfghjkl', 'zxcvbnm'];
  const caps: KeyCap[] = [
    { kind: 'caret-left', row: 0, left: 0, width: 15, label: '' },
    { kind: 'caret-right', row: 0, left: 85, width: 15, label: '' },
  ];
  rows.forEach((chars, r) => {
    const row = r + 1;
    const first = row === 2 ? 5 + (10 - chars.length) * 5 + (chars.length === 9 ? -5 : 0) : row === 3 ? 15 : 0;
    if (row === 3) caps.push({ kind: 'shift', row, left: 0, width: 15, label: '' });
    Array.from(chars).forEach((c, i) => caps.push({ kind: 'char', row, left: first + 10 * i, width: 10, label: !t.digits && t.shift !== 'off' ? c.toUpperCase() : c }));
    if (row === 3) caps.push({ kind: 'delete', row, left: 85, width: 15, label: '' });
  });
  caps.push({ kind: 'layer', row: 4, left: 0, width: 25, label: t.digits ? 'ABC' : '123' });
  caps.push(t.target !== 'icon' ? { kind: 'space', row: 4, left: 25, width: 50, label: 'space' } : { kind: 'space', row: 4, left: 25, width: 50, label: '' });
  caps.push({ kind: 'return', row: 4, left: 75, width: 25, label: 'OK' });
  return caps;
}

const NAME_LIMIT = 16;
const keyLimit = (t: Typing) => (t.target === 'icon' ? MONOGRAM_LIMIT : NAME_LIMIT);

function typeKey(s: State, t: Typing, cap: KeyCap): State {
  const put = (next: Typing) => ({ ...s, keyboard: next });
  switch (cap.kind) {
    case 'return':
      if (t.target === 'icon') return { ...s, keyboard: null, icon: t.text ? `text:${t.text}` : s.icon };
      if (t.target === 'saveas') return saveAs({ ...s, keyboard: null }, t.text.trim() || s.name);
      return { ...s, keyboard: null, name: t.text.trim() || s.name };
    case 'delete':
      return t.cursor ? put({ ...t, text: t.text.slice(0, t.cursor - 1) + t.text.slice(t.cursor), cursor: t.cursor - 1 }) : s;
    case 'shift':
      return put({ ...t, shift: t.shift === 'off' ? 'once' : t.shift === 'once' ? 'locked' : 'off' });
    case 'layer':
      return put({ ...t, digits: !t.digits });
    case 'caret-left':
      return put({ ...t, cursor: Math.max(0, t.cursor - 1) });
    case 'caret-right':
      return put({ ...t, cursor: Math.min(t.text.length, t.cursor + 1) });
  }
  if (t.target === 'icon' && cap.kind === 'space') return s;
  const c = cap.kind === 'space' ? ' ' : cap.label;
  if (t.text.length >= keyLimit(t)) return s;
  return put({ ...t, text: t.text.slice(0, t.cursor) + c + t.text.slice(t.cursor), cursor: t.cursor + 1, shift: t.shift === 'once' ? 'off' : t.shift });
}

function pressKeyboard(s: State, t: Typing, key: CameraKey): State {
  const caps = keyCaps(t);
  const at = caps[t.focus];
  const centre = (c: KeyCap) => c.left + c.width / 2;
  const nearest = (row: number) => {
    let best = -1;
    caps.forEach((c, i) => {
      if (c.row === row && (best < 0 || Math.abs(centre(c) - centre(at)) < Math.abs(centre(caps[best]) - centre(at)))) best = i;
    });
    return best;
  };
  const inRow = caps.map((c, i) => [c, i] as const).filter(([c]) => c.row === at.row).map(([, i]) => i);
  const k = inRow.indexOf(t.focus);
  switch (key) {
    case 'left':
      return { ...s, keyboard: { ...t, focus: inRow[(k + inRow.length - 1) % inRow.length] } };
    case 'right':
      return { ...s, keyboard: { ...t, focus: inRow[(k + 1) % inRow.length] } };
    case 'up':
      return at.row > 0 ? { ...s, keyboard: { ...t, focus: nearest(at.row - 1) } } : s;
    case 'down':
      return at.row < 4 ? { ...s, keyboard: { ...t, focus: nearest(at.row + 1) } } : s;
    case 'ok':
      return typeKey(s, t, at);
    case 'dial-':
      return typeKey(s, t, caps[0]);
    case 'dial+':
      return typeKey(s, t, caps[1]);
    case 'back':
      return { ...s, keyboard: null };
  }
  return s;
}

const typing = (text: string, target: TypingTarget): Typing => ({ text, cursor: text.length, focus: 4, shift: target === 'icon' ? 'locked' : 'off', digits: false, target });

function DialGlyph() {
  return (
    <svg width="26" height="22" viewBox="0 0 26 22" fill="none" stroke="currentColor" stroke-width="2">
      <ellipse cx="13" cy="7" rx="9" ry="4" />
      <path d="M4 7v5c0 2.2 4 4 9 4s9-1.8 9-4V7M8 19l-2.5 1.5M18 19l2.5 1.5" />
    </svg>
  );
}

function KeyLabel(props: { cap: KeyCap; t: Typing }) {
  const c = props.cap;
  switch (c.kind) {
    case 'caret-left':
      return <span class="flex items-center gap-[8px]"><Chevron dir="left" size={18} /><DialGlyph /></span>;
    case 'caret-right':
      return <span class="flex items-center gap-[8px]"><DialGlyph /><Chevron dir="right" size={18} /></span>;
    case 'shift':
      return (
        <svg width="26" height="26" viewBox="0 0 24 24" fill={props.t.shift === 'off' ? 'none' : 'currentColor'} stroke="currentColor" stroke-width="2" stroke-linejoin="round">
          <path d="M12 3 3 12h5v8h8v-8h5z" />
          {props.t.shift === 'locked' ? <path d="M8 23h8" /> : null}
        </svg>
      );
    case 'delete':
      return (
        <svg width="30" height="24" viewBox="0 0 30 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linejoin="round">
          <path d="M9 3h18v18H9L1 12z" />
          <path d="m14 8 8 8m0-8-8 8" />
        </svg>
      );
  }
  return <>{c.label}</>;
}

function Keyboard(props: { s: State; t: Typing; update: (s: State) => void }) {
  const t = () => props.t;
  const icon = () => t().target === 'icon';
  const ROW_TOP = [6, 54, 124, 194, 264];
  const ROW_H = [38, 60, 60, 60, 60];
  const caps = () => keyCaps(t());
  return (
    <div class="absolute inset-0 flex flex-col" style={{ 'text-transform': 'none' }}>
      <div class="flex h-[48px] items-center gap-[14px] px-[18px]" style={{ background: 'var(--m-title)' }}>
        <button type="button" class="flex size-[32px] items-center justify-center text-white/70" onClick={() => props.update({ ...props.s, keyboard: null })}><Chevron dir="left" size={22} /></button>
        <span class="text-[21px] font-medium tracking-[0.18em] text-white/75">{icon() ? 'ICON LETTERS' : t().target === 'saveas' ? 'SAVE AS NEW' : 'FILM NAME'}</span>
      </div>
      <div class="flex grow items-start gap-[14px] px-[20px] pt-[16px]" style={{ background: 'var(--m-text-area)' }}>
        <Show when={icon()}>
          <FilmIcon slug={`text:${t().text}`} size={52} color="#fff" />
        </Show>
        <span class="text-[29px] leading-[40px] whitespace-pre text-white" style={{ 'font-variant': icon() ? 'small-caps' : undefined }}>
          {t().text.slice(0, t().cursor)}
          <span class="inline-block h-[30px] w-[3px] translate-y-[5px] bg-[#0a6bdc]" />
          {t().text.slice(t().cursor)}
        </span>
      </div>
      <div class="relative h-[330px] shrink-0" style={{ background: 'var(--m-tray)' }}>
        <For each={caps()}>
          {(cap, i) => {
            const special = cap.kind !== 'char' && cap.kind !== 'space';
            const focused = () => t().focus === i();
            return (
              <button
                type="button"
                class="absolute flex items-center justify-center rounded-[7px] text-white"
                style={{
                  left: `${6 + cap.left * 7.08 + 3}px`,
                  width: `${cap.width * 7.08 - 6}px`,
                  top: `${ROW_TOP[cap.row]}px`,
                  height: `${ROW_H[cap.row]}px`,
                  background: cap.kind === 'return' ? 'var(--m-confirm)' : special ? 'var(--m-key-special)' : 'var(--m-key)',
                  'box-shadow': focused() ? 'var(--m-key-focus)' : 'var(--m-key-edge)',
                  'font-size': cap.kind === 'char' ? '29px' : '21px',
                  'font-variant': icon() && cap.kind === 'char' ? 'small-caps' : undefined,
                  'letter-spacing': cap.kind === 'layer' ? '0.12em' : undefined,
                }}
                onClick={() => props.update(typeKey({ ...props.s, keyboard: { ...t(), focus: i() } }, { ...t(), focus: i() }, cap))}
              >
                <KeyLabel cap={cap} t={t()} />
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}

export function press(s: State, key: CameraKey): State {
  const control = CONTROLS[s.focus];
  const f = s.film;
  const d = key === 'left' || key === 'down' || key === 'dial-' ? -1 : 1;
  if (s.keyboard) return pressKeyboard(s, s.keyboard, key);
  if (s.numbers) return pressNumbers(s, s.numbers, key);
  if (s.picker) {
    const p = s.picker;
    const options = pickerOptions(p);
    const at = pickerStep(p.at, options.length, key, p.kind === 'icon' ? ICON_COLUMNS : 1);
    if (at !== undefined) return { ...s, picker: { ...p, at } };
    switch (key) {
      case 'ok':
        return choose(s, p, options[p.at]);
      case 'back':
        return { ...s, picker: null };
    }
    return s;
  }
  switch (key) {
    case 'q':
      return { ...s, compare: s.compare ? null : { b: 'opened', split: SPLIT } };
    case 'fn':
      return { ...s, custom: !s.custom };
    case 'play':
      if (!s.crop) return { ...s, crop: 1, area: 'strip', railOn: null, inside: false };
      return { ...s, crop: 0, area: s.area === 'strip' || s.area === 'create' ? 'custom' : s.area, railOn: s.railOn === 'raw' ? 'custom' : s.railOn };
  }
  const step = key === 'left' || key === 'dial-' ? -1 : 1;
  if (s.area === 'rail') {
    const list = railSections(s);
    const at = list.indexOf(currentSection(s));
    switch (key) {
      case 'up':
      case 'down':
      case 'dial-':
      case 'dial+': {
        const next = list[at + (key === 'up' || key === 'dial-' ? -1 : 1)];
        if (!next) return s;
        if (next === 'film' || next === 'raw' || next === 'custom') return { ...s, railOn: next, scrollTo: undefined };
        return { ...s, railOn: null, focus: firstOf(next), header: false, scrollTo: undefined };
      }
      case 'right':
      case 'ok':
        return enterSection(s, currentSection(s));
    }
    return s;
  }
  if (s.area !== 'rows') {
    const area = s.area;
    const grid = metaRows(s);
    const r = grid.findIndex((row) => row.includes(area));
    const row = grid[r];
    switch (key) {
      case 'up':
        return r > 0 ? { ...s, area: grid[r - 1][Math.min(row.indexOf(area), grid[r - 1].length - 1)] } : s;
      case 'down':
        return r < grid.length - 1 ? { ...s, area: grid[r + 1][Math.min(row.indexOf(area), grid[r + 1].length - 1)] } : { ...s, area: 'rows', focus: 0, header: true };
      case 'back':
        return { ...s, area: 'rail', railOn: META_SECTION[area] as 'film' | 'raw' | 'custom' };
    }
    if (row.length > 1 && (key === 'left' || key === 'right')) {
      const next = row[row.indexOf(area) + step];
      return next ? { ...s, area: next } : s;
    }
    switch (area) {
      case 'strip':
        if (key === 'ok') return { ...s, area: 'create' };
        return { ...s, crop: Math.max(1, Math.min(RAWS.length, s.crop + step)), created: 0 };
      case 'create':
        return key === 'ok' ? { ...s, created: s.crop } : s;
      case 'custom':
        return { ...s, custom: !s.custom };
      case 'open':
        return key === 'ok' ? openPicker(s, { kind: 'open' }) : s;
      case 'save':
        return key === 'ok' ? save(s) : s;
      case 'saveas':
        return key === 'ok' ? { ...s, keyboard: typing(nextName(s.name), 'saveas') } : s;
      case 'reflect':
        return key === 'ok' ? { ...s, raw: RAW_DEFAULT, custom: true } : s;
      case 'push':
      case 'space':
      case 'filetype':
      case 'size':
      case 'quality':
      case 'lmo': {
        if (key === 'ok') return s;
        const k = RAW_OPTIONS.findIndex((o) => o.key === area);
        const raw = [...s.raw];
        raw[k] = Math.max(0, Math.min(RAW_OPTIONS[k].values.length - 1, raw[k] + step));
        return { ...s, raw };
      }
      case 'icon':
        return key === 'ok' ? openPicker(s, { kind: 'icon' }) : s;
      case 'name':
        return key === 'ok' ? { ...s, keyboard: typing(s.name, 'name') } : s;
    }
  }
  if (s.inside) {
    /* Inside the tone curve, OK takes it full screen and back. */
    if (key === 'back') return s.full ? { ...s, full: false } : { ...s, inside: false, bend: false };
    if (key === 'ok') return control.kind === 'curve' ? { ...s, full: !s.full } : { ...s, inside: false };
    if (control.kind === 'curve') {
      if (key === 'left' || key === 'right') return { ...s, knot: Math.max(0, Math.min(9, s.knot + d)), bend: false };
      if (key === 'dial-' || key === 'dial+') {
        const tilt = [...f.tilt];
        tilt[s.knot] = Math.round((tilt[s.knot] + d * 0.1) * 10) / 10;
        return { ...s, bend: true, film: { ...f, tilt } };
      }
      const knots = [...f.knots];
      knots[s.knot] = fromDisp(disp(knots[s.knot]) + d / 70);
      return { ...s, bend: false, film: { ...f, knots } };
    }
    if (key === 'left' || key === 'right') return { ...s, band: Math.max(0, Math.min(4, s.band + d)) };
    const bands = [...f.bands];
    bands[s.band] = Math.max(-100, Math.min(100, bands[s.band] + d * 5));
    return { ...s, film: { ...f, bands } };
  }
  const first = s.focus === firstOf(control.tool);
  switch (key) {
    case 'up':
      if (!s.header && first) return { ...s, header: true, scrollTo: undefined };
      if (s.focus === 0) return { ...s, area: 'custom', header: false };
      return { ...s, focus: s.focus - 1, header: false, scrollTo: undefined };
    case 'down':
      if (s.header) return { ...s, header: false, scrollTo: undefined };
      if (s.focus === CONTROLS.length - 1) return s;
      return { ...s, focus: s.focus + 1, header: CONTROLS[s.focus + 1].tool !== control.tool, scrollTo: undefined };
    case 'back':
      return { ...s, area: 'rail', railOn: null, header: false };
    case 'ok':
      if (s.header) return openPicker(s, { kind: 'copy', tool: control.tool });
      if (control.kind === 'numbers') return { ...s, numbers: { tool: control.tool, cell: 1 } };
      return control.kind === 'scale' || control.kind === 'chips' ? s : { ...s, inside: true };
  }
  if (s.header) return s;
  if (control.kind === 'chips') return { ...s, primary: Math.max(0, Math.min(2, s.primary + step)) };
  if (control.kind !== 'scale') return s;
  const c = scaleOf(control, s);
  return { ...s, film: c.put(f, Math.max(c.min, Math.min(c.max, c.get(f) + step * c.step))) };
}

function ToolIcon(props: { tool: Section; color: string }) {
  const stroke = () => ({ fill: 'none', stroke: props.color, 'stroke-width': 2.5, 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const });
  return (
    <svg width="32" height="32" viewBox="0 0 34 34">
      <Show when={props.tool === 'film'}>
        <rect x="5" y="5" width="24" height="24" rx="2" {...stroke()} />
        <path d="M5 10h24M5 24h24" {...stroke()} stroke-opacity="0.6" />
        <path d="M9 7.5h2M15 7.5h2M21 7.5h2M9 26.5h2M15 26.5h2M21 26.5h2" {...stroke()} stroke-width="2" />
      </Show>
      <Show when={props.tool === 'raw'}>
        <rect x="4" y="8" width="26" height="18" rx="2" {...stroke()} />
        <path d="M14 12.5v9l7.5-4.5z" fill={props.color} />
      </Show>
      <Show when={props.tool === 'custom'}>
        <path d="M6 10h22M6 17h22M6 24h22" {...stroke()} stroke-opacity="0.5" />
        <circle cx="12" cy="10" r="3.2" fill={props.color} />
        <circle cx="22" cy="17" r="3.2" fill={props.color} />
        <circle cx="15" cy="24" r="3.2" fill={props.color} />
      </Show>
      <Show when={props.tool === 'curve'}>
        <rect x="5" y="5" width="24" height="24" rx="3" {...stroke()} stroke-opacity="0.5" />
        <path d="M7 27C14 26 13 9 27 7" {...stroke()} />
      </Show>
      <Show when={props.tool === 'color'}>
        <circle cx="17" cy="17" r="11" {...stroke()} />
        <path d="M17 6a11 11 0 0 1 0 22z" fill={props.color} />
      </Show>
      <Show when={props.tool === 'mix'}>
        <circle cx="12" cy="13" r="6.5" {...stroke()} />
        <circle cx="22" cy="13" r="6.5" {...stroke()} />
        <circle cx="17" cy="22" r="6.5" {...stroke()} />
      </Show>
      <Show when={props.tool === 'tone'}>
        <path d="M6 24v-4M12 24v-9M18 24v-13M24 24v-7M29 24v-3" {...stroke()} />
        <path d="M5 28h24" {...stroke()} stroke-opacity="0.5" />
      </Show>
      <Show when={props.tool === 'channels'}>
        <path d="M6 27C13 25 15 12 28 8" {...stroke()} />
        <path d="M6 27C15 26 18 13 28 10" {...stroke()} stroke-opacity="0.55" />
      </Show>
    </svg>
  );
}

function Swatch(props: { film: Film; mix: number }) {
  const after = () => {
    const m = mixMatrix(props.film);
    const l = MIX[props.mix].linear;
    const rgb = [0, 1, 2].map((r) => encode(Math.max(0, Math.min(1, m[r * 3] * l[0] + m[r * 3 + 1] * l[1] + m[r * 3 + 2] * l[2]))) * 255);
    return `rgb(${rgb.map(Math.round).join(' ')})`;
  };
  return (
    <span
      class="fm-round block size-[18px] shrink-0 rounded-full"
      style={{ background: `linear-gradient(90deg, ${MIX[props.mix].color} 50%, ${after()} 50%)`, 'box-shadow': 'inset 0 0 0 2px rgb(255 255 255 / 0.35)' }}
    />
  );
}

function ExpandIcon(props: { full?: boolean }) {
  return (
    <svg width="22" height="22" viewBox="0 0 22 22">
      <path
        d={props.full ? 'M8 3v5H3M14 3v5h5M8 19v-5H3M14 19v-5h5' : 'M3 8V3h5M19 8V3h-5M3 14v5h5M19 14v5h-5'}
        fill="none"
        stroke="#fff"
        stroke-width="2.2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}


type StripStyle = 'frames' | 'contact' | 'film';
const STRIP_STYLES: [StripStyle, string][] = [
  ['frames', 'Frames'],
  ['contact', 'Contact sheet'],
  ['film', 'Film strip'],
];
const PICTURE_WIDTH = W - PICTURE_LEFT;

function Thumb(props: { raw: number; width: number; height: number; radius: number; style?: JSX.CSSProperties }) {
  const raw = () => RAWS[props.raw];
  return (
    <span
      class="block shrink-0 bg-cover"
      style={{
        width: `${props.width}px`,
        height: `${props.height}px`,
        'border-radius': `${props.radius}px`,
        'background-image': `url(${PHOTO})`,
        'background-size': `${raw().zoom * 100}% auto`,
        'background-position': `${raw().x * 100}% ${raw().y * 100}%`,
        ...props.style,
      }}
    />
  );
}

/** The RAW files on the SD card, across the top of the picture. The chosen
 * one stays in the middle. */
function stripMetrics(style: StripStyle) {
  return style === 'frames'
    ? { w: 120, h: 80, gap: 8, radius: 2, band: 104 }
    : style === 'contact'
      ? { w: 108, h: 72, gap: 8, radius: 1, band: 110 }
      : { w: 120, h: 80, gap: 4, radius: 0, band: 108 };
}

function RawStrip(props: { s: State; set: (s: State) => void }) {
  const s = () => props.s;
  const focused = () => s().area === 'strip';
  const metrics = () => stripMetrics(s().strip);
  const offset = () => {
    const m = metrics();
    /* The first file at the left edge; the strip moves only to keep the chosen file in view. */
    const end = (s().crop - 1) * (m.w + m.gap) + m.w;
    return 8 - Math.max(0, end - (PICTURE_WIDTH - 16));
  };
  const choose = (i: number) => props.set({ ...s(), crop: i + 1, area: 'strip', railOn: null, created: 0 });
  const band = (): JSX.CSSProperties =>
    s().strip === 'frames'
      ? { background: 'linear-gradient(rgb(0 0 0 / 0.7), rgb(0 0 0 / 0))' }
      : s().strip === 'contact'
        ? { ...COLUMN, background: 'rgb(18 18 20 / 0.86)' }
        : { background: '#0b0b0b' };
  return (
    <div class="absolute top-0 overflow-hidden" style={{ left: `${PICTURE_LEFT}px`, width: `${PICTURE_WIDTH}px`, height: `${metrics().band}px`, ...band() }}>
      <Show when={s().strip === 'film'}>
        <div class="absolute inset-x-0 top-[5px] h-[6px]" style={{ background: 'repeating-linear-gradient(90deg, transparent 0 7px, rgb(255 255 255 / 0.55) 7px 15px, transparent 15px 22px)', 'border-radius': '1px' }} />
        <div class="absolute inset-x-0 bottom-[5px] h-[6px]" style={{ background: 'repeating-linear-gradient(90deg, transparent 0 7px, rgb(255 255 255 / 0.55) 7px 15px, transparent 15px 22px)', 'border-radius': '1px' }} />
      </Show>
      <div
        class="absolute flex"
        style={{
          top: s().strip === 'film' ? '14px' : '8px',
          gap: `${metrics().gap}px`,
          transform: `translateX(${offset()}px)`,
          transition: 'transform 260ms cubic-bezier(0.22, 1, 0.36, 1)',
        }}
      >
        <For each={RAWS}>
          {(raw, i) => {
            const chosen = () => s().crop === i() + 1;
            const m = metrics;
            return (
              <button type="button" class="flex flex-col items-center gap-[4px]" onClick={() => choose(i())}>
                <Thumb
                  raw={i()}
                  width={m().w}
                  height={m().h}
                  radius={m().radius}
                  style={{
                    opacity: chosen() ? 1 : 0.5,
                    'box-shadow':
                      chosen() && s().strip !== 'film'
                        ? `0 0 0 2px #000, 0 0 0 ${focused() ? 5 : 4}px ${focused() ? '#fff' : 'rgb(255 255 255 / 0.6)'}`
                        : undefined,
                  }}
                />
                <Show when={s().strip === 'contact'}>
                  <span class={`font-medium tabular-nums ${SMALL}`} style={{ color: chosen() ? '#fff' : 'rgb(255 255 255 / 0.5)' }}>
                    {raw.name.slice(4, 8)}
                  </span>
                </Show>
              </button>
            );
          }}
        </For>
      </div>
      <Show when={s().strip === 'film' && focused()}>
        <div
          class="pointer-events-none absolute top-[14px]"
          style={{ left: `${offset() + (s().crop - 1) * (metrics().w + metrics().gap)}px`, width: `${metrics().w}px`, height: `${metrics().h}px`, 'box-shadow': 'inset 0 0 0 3px #fff', transition: 'left 260ms cubic-bezier(0.22, 1, 0.36, 1)' }}
        />
      </Show>
    </div>
  );
}

/** The film editor's own layers over the picture: the rail, the scroller,
 * the compare divider, RAW strip, pickers and key hints. */
/* ---- COLOR MIX scope and COLOR wheel ------------------------------------------------ */

const SCOPE = 236;
const SCOPE_R = SCOPE / 2 - 10;
/** A pure primary sits on this ring; out from it is more saturated. */
const RING = SCOPE_R * 0.7;
const PRIMARY_INKS = ['#ff453a', '#30d158', '#0a84ff'];
const scopeAt = (angle: number, radius: number): [number, number] => [SCOPE / 2 + radius * Math.cos(angle), SCOPE / 2 - radius * Math.sin(angle)];
/** Hue as the angle of (Cb, Cr). */
const hueAngle = (r: number, g: number, b: number) => {
  const y = 0.299 * r + 0.587 * g + 0.114 * b;
  return Math.atan2(0.713 * (r - y), 0.564 * (b - y));
};
const HOME = [hueAngle(1, 0, 0), hueAngle(0, 1, 0), hueAngle(0, 0, 1)];
const HUE_MARKS = Array.from({ length: 12 }, (_, i) => {
  const [r, g, b] = hsv(i * 30, 1, 1);
  return { angle: hueAngle(r, g, b), ink: `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})` };
});

/** COLOR MIX's vectorscope: the picture's colors after the matrix, and a handle per primary to drag. */
function Scope(props: { film: Film; crop: number; chosen: number; choose: (p: number) => void; set: (film: Film) => void }) {
  let dots: HTMLCanvasElement | undefined;
  let box: HTMLDivElement | undefined;
  const [src, setSrc] = createSignal<Source>();
  createEffect(
    () => props.crop,
    (crop) => {
      void source(crop).then(setSrc);
    },
  );
  createEffect(
    () => [mixMatrix(props.film), src()] as const,
    ([m, s]) => {
      const g = dots?.getContext('2d');
      if (!g || !s) return;
      g.clearRect(0, 0, SCOPE, SCOPE);
      g.fillStyle = 'rgb(255 255 255 / 0.22)';
      const px = s.pixels;
      for (let p = 0; p < px.length; p += 4 * 41) {
        const r = DECODE[px[p]];
        const gg = DECODE[px[p + 1]];
        const b = DECODE[px[p + 2]];
        const c = [0, 1, 2].map((k) => encode(Math.max(0, Math.min(1, m[k * 3] * r + m[k * 3 + 1] * gg + m[k * 3 + 2] * b))));
        const hi = Math.max(...c);
        if (hi < 0.04) continue;
        const [x, y] = scopeAt(hueAngle(c[0], c[1], c[2]), ((hi - Math.min(...c)) / hi) * RING);
        g.fillRect(x, y, 1.5, 1.5);
      }
    },
  );
  const handle = (i: number) => scopeAt(HOME[i] + (props.film.primaries[i].hue * Math.PI) / 180, RING * (1 + props.film.primaries[i].sat / 100));
  const moveTo = (i: number, x: number, y: number) => {
    let hue = ((Math.atan2(SCOPE / 2 - y, x - SCOPE / 2) - HOME[i]) * 180) / Math.PI;
    hue = ((hue + 540) % 360) - 180;
    const sat = (Math.hypot(x - SCOPE / 2, y - SCOPE / 2) / RING - 1) * 100;
    const scales = mixScales(i);
    const f = scales[0].put(props.film, Math.round(hue));
    props.set(scales[1].put(f, Math.round(Math.max(-100, Math.min(100, sat)))));
  };
  const local = (e: PointerEvent): [number, number] => {
    const r = box!.getBoundingClientRect();
    const k = SCOPE / r.width;
    return [(e.clientX - r.left) * k, (e.clientY - r.top) * k];
  };
  return (
    <div
      ref={box}
      class="relative mx-auto touch-none"
      style={{ width: `${SCOPE}px`, height: `${SCOPE}px` }}
      onPointerDown={(e) => {
        const [x, y] = local(e);
        const near = [0, 1, 2].reduce((best, i) => (Math.hypot(handle(i)[0] - x, handle(i)[1] - y) < Math.hypot(handle(best)[0] - x, handle(best)[1] - y) ? i : best), props.chosen);
        props.choose(near);
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
        const [x, y] = local(e);
        moveTo(props.chosen, x, y);
      }}
    >
      <div class="fm-round absolute inset-[10px] rounded-full" style={{ background: 'var(--m-well)', 'box-shadow': 'var(--m-well-edge), 0 0 0 1px rgb(255 255 255 / 0.14)' }} />
      <canvas ref={dots} width={SCOPE} height={SCOPE} class="absolute inset-0" />
      <svg class="absolute inset-0" width={SCOPE} height={SCOPE}>
        <circle cx={SCOPE / 2} cy={SCOPE / 2} r={RING} fill="none" stroke="rgb(255 255 255 / 0.12)" />
        <For each={HUE_MARKS}>{(m) => <circle cx={scopeAt(m.angle, SCOPE_R - 6)[0]} cy={scopeAt(m.angle, SCOPE_R - 6)[1]} r="3.5" fill={m.ink} />}</For>
        <polygon points={HOME.map((a) => scopeAt(a, RING).join(',')).join(' ')} fill="none" stroke="rgb(255 255 255 / 0.3)" stroke-dasharray="4 4" />
        <polygon points={[0, 1, 2].map((i) => handle(i).join(',')).join(' ')} fill="rgb(255 255 255 / 0.05)" stroke="rgb(255 255 255 / 0.75)" stroke-width="1.5" />
        <For each={[0, 1, 2]}>
          {(i) => <circle cx={handle(i)[0]} cy={handle(i)[1]} r={props.chosen === i ? 13 : 10} fill={PRIMARY_INKS[i]} stroke="#fff" stroke-width={props.chosen === i ? 3 : 1.5} />}
        </For>
      </svg>
    </div>
  );
}

/** COLOR's wheel: the handle's angle is HUE, its distance SATURATION; WARM/COOL stretches the ring along orange-blue. */
function ColorWheel(props: { film: Film; set: (film: Film) => void }) {
  const D = 200;
  const c = D / 2;
  const R = 62;
  let box: HTMLDivElement | undefined;
  const radius = () => R * (1 + props.film.saturation / 100);
  /* Up is no hue shift; the handle turns one degree per degree of HUE, all the way around. */
  const angle = () => ((props.film.hue - 90) * Math.PI) / 180;
  const stretch = () => 1 + props.film.warmth / 100;
  const axis = (Math.atan2(0.8, -0.6) * 180) / Math.PI;
  const set = (e: PointerEvent) => {
    const r = box!.getBoundingClientRect();
    const x = ((e.clientX - r.left) * D) / r.width - c;
    const y = ((e.clientY - r.top) * D) / r.height - c;
    const hue = Math.round((((Math.atan2(y, x) * 180) / Math.PI + 90 + 540) % 360) - 180);
    const saturation = Math.round(Math.max(-100, Math.min(100, (Math.hypot(x, y) / R - 1) * 100)) / 5) * 5;
    props.set(COLOR_SCALES[0].put(COLOR_SCALES[1].put(props.film, hue), saturation));
  };
  return (
    <div
      ref={box}
      class="relative mx-auto touch-none"
      style={{ width: `${D}px`, height: `${D}px` }}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        set(e);
      }}
      onPointerMove={(e) => e.currentTarget.hasPointerCapture(e.pointerId) && set(e)}
    >
      <div
        class="fm-round absolute inset-[6px] rounded-full"
        style={{ background: 'conic-gradient(from 0deg, #ff3b30, #ff9500, #ffd60a, #30d158, #00c7be, #0a84ff, #bf5af2, #ff2d78, #ff3b30)', transform: `rotate(${props.film.hue}deg)`, 'mask-image': 'radial-gradient(closest-side, transparent 80%, #000 82%)' }}
      />
      <svg class="absolute inset-0" width={D} height={D}>
        <circle cx={c} cy={c} r={R} fill="none" stroke="rgb(255 255 255 / 0.3)" stroke-dasharray="4 4" />
        <ellipse cx={c} cy={c} rx={radius() * stretch()} ry={radius()} transform={`rotate(${axis} ${c} ${c})`} fill="rgb(255 255 255 / 0.06)" stroke="rgb(255 255 255 / 0.75)" stroke-width="1.5" />
        <line x1={c} y1={c} x2={c + radius() * Math.cos(angle())} y2={c + radius() * Math.sin(angle())} stroke="#fff" stroke-width="2" stroke-linecap="round" />
        <circle cx={c + radius() * Math.cos(angle())} cy={c + radius() * Math.sin(angle())} r="11" fill="var(--m-mark)" stroke="#fff" stroke-width="2.5" />
      </svg>
    </div>
  );
}

/** A matrix as numbers over the column: rows are outputs, columns inputs. */
function NumbersSheet(props: { s: State; update: (s: State) => void }) {
  const n = () => props.s.numbers!;
  const size = () => MATRIX_SIZE[n().tool];
  const labels = () => (n().tool === 'mix' ? ['R', 'G', 'B'] : ['Cb', 'Cr']);
  const values = () => cells(props.s.film, n().tool);
  return (
    <div class="absolute inset-y-0 flex flex-col px-[12px] pt-[10px]" style={{ ...COLUMN, left: `${RAIL}px`, width: `${SCROLLER}px` }}>
      <div class="mb-[10px] flex items-center gap-[6px]">
        <button type="button" class="flex size-[32px] items-center justify-center text-white/70" onClick={() => props.update({ ...props.s, numbers: null })}>
          <Chevron dir="left" size={22} />
        </button>
        <span class={`font-semibold ${HEADING}`}>{TOOL_NAMES[n().tool]}</span>
        <span class={`ml-auto text-white/55 ${SMALL}`}>NUMBERS</span>
      </div>
      <div class="grid gap-[8px]" style={{ 'grid-template-columns': `30px repeat(${size()}, 1fr)` }}>
        <span />
        <For each={labels()}>{(l) => <span class={`text-center text-white/50 ${SMALL}`}>{l}</span>}</For>
        <For each={labels()}>
          {(rowLabel, r) => (
            <>
              <span class={`flex items-center text-white/50 ${SMALL}`}>{rowLabel}</span>
              <For each={labels()}>
                {(_, col) => {
                  const cell = () => r() * size() + col();
                  const fixed = () => fixedCell(n().tool, cell());
                  const on = () => n().cell === cell();
                  return (
                    <div
                      class="flex h-[54px] touch-none items-center justify-center rounded-[10px] text-[22px] font-medium tabular-nums"
                      style={
                        fixed()
                          ? { color: 'rgb(255 255 255 / 0.45)', 'box-shadow': 'inset 0 0 0 1px rgb(255 255 255 / 0.14)' }
                          : { ...TRACK(on()), color: '#fff' }
                      }
                      onPointerDown={(e) => {
                        if (fixed()) return;
                        const y0 = e.clientY;
                        const v0 = values()[cell()];
                        const at = cell();
                        props.update({ ...props.s, numbers: { ...n(), cell: at } });
                        e.currentTarget.setPointerCapture(e.pointerId);
                        const el = e.currentTarget;
                        const move = (ev: PointerEvent) => props.update({ ...props.s, film: putCell(props.s.film, n().tool, at, v0 + (y0 - ev.clientY) / 300) });
                        const up = () => {
                          el.removeEventListener('pointermove', move);
                          el.removeEventListener('pointerup', up);
                        };
                        el.addEventListener('pointermove', move);
                        el.addEventListener('pointerup', up);
                      }}
                    >
                      {values()[cell()].toFixed(2).replace('-', '−')}
                    </div>
                  );
                }}
              </For>
            </>
          )}
        </For>
      </div>
    </div>
  );
}

export function Editor(props: {
  s: State;
  set: (s: State) => void;
  histogram: number[];
  /** The rail and the scroller, as presented. */
  column?: JSX.CSSProperties;
  /** The key hints at the bottom right, as presented. */
  chrome?: JSX.CSSProperties;
}) {
  const s = () => props.s;
  let scroller: HTMLDivElement | undefined;
  const sections: Partial<Record<Section, HTMLElement>> = {};
  const rows: HTMLElement[] = [];
  const [inView, setInView] = createSignal<Section>(CONTROLS[props.s.focus].tool);
  const update = (next: State) => props.set(next);
  const scrollToTool = (tool: Section, smooth: boolean) => {
    const section = sections[tool];
    if (scroller && section) scroller.scrollTo({ top: section.offsetTop - 6, behavior: smooth ? 'smooth' : 'auto' });
  };
  onSettled(() => {
    const tool = props.s.scrollTo;
    if (tool) scrollToTool(tool, false);
    spy();
  });
  /* The editor opens at the top; later focus moves keep the focused row in view. */
  let opened = false;
  createEffect(
    () => s().focus,
    (focus) => {
      if (!opened) {
        opened = true;
        return;
      }
      const row = rows[focus];
      if (s().scrollTo || !row || !scroller) return;
      /* Only the scroller moves: scrollIntoView would scroll the page too. */
      const top = row.offsetTop;
      const bottom = top + row.offsetHeight;
      if (top < scroller.scrollTop + 8) scroller.scrollTo({ top: top - 60, behavior: 'smooth' });
      else if (bottom > scroller.scrollTop + scroller.clientHeight - 8)
        scroller.scrollTo({ top: bottom - scroller.clientHeight + 16, behavior: 'smooth' });
    },
  );
  const spy = () => {
    if (!scroller) return;
    const list = railSections(s());
    let current: Section = list[0];
    for (const tool of list) {
      const section = sections[tool];
      if (section && section.offsetTop - 60 <= scroller.scrollTop) current = tool;
    }
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 4) current = list[list.length - 1];
    setInView(current);
  };
  const focusControl = (i: number) =>
    (s().focus !== i || s().area !== 'rows' || s().header) && update({ ...s(), focus: i, area: 'rows', header: false, inside: false, bend: false, scrollTo: undefined });
  const isFocused = (i: number) => s().area === 'rows' && !s().header && s().focus === i;
  const headerFocused = (tool: Tool) => s().area === 'rows' && s().header && CONTROLS[s().focus].tool === tool;
  /* Inside a tool its frame is the accent; focused from outside it is lit. */
  const frame = (i: number): JSX.CSSProperties =>
    isFocused(i) ? (s().inside ? { ...TRACK(true), 'box-shadow': `inset 0 0 0 3px ${ACCENT}` } : TRACK(true)) : {};
  /* On the rail, UP and DOWN bring each tool to the top of the scroller. */
  createEffect(
    () => (s().area === 'rows' ? undefined : currentSection(s())),
    (tool) => {
      if (tool) scrollToTool(tool, true);
    },
  );
  const edited = (tool: Tool) => !same(GROUPS[tool](s().film), GROUPS[tool](s().opened));
  const scaleRow = (i: number, c: Scale, width: number, _nameWidth?: number) => (
    <div ref={(element) => (rows[i] = element)} class="py-[2px]">
      <Dial
        name={c.name}
        value={c.get(s().film)}
        min={c.min}
        max={c.max}
        step={c.step}
        width={width}
        focused={isFocused(i)}
        changed={c.get(s().film) !== c.get(s().opened)}
        track={c.track}
        onFocus={() => focusControl(i)}
        set={(v) => update({ ...s(), focus: i, film: c.put(s().film, v) })}      />
    </div>
  );
  const index = (tool: Tool, nth = 0) => CONTROLS.findIndex((c) => c.tool === tool) + nth;
  /* A quiet row at a tool's foot: OK or a tap shows its matrix as numbers. */
  const numbersRow = (i: number, tool: 'mix' | 'color') => (
    <button
      type="button"
      ref={(element) => (rows[i] = element)}
      class={`my-[2px] flex h-[44px] w-full items-center justify-between rounded-[10px] px-[12px] text-white/50 ${TEXT}`}
      style={isFocused(i) ? { ...TRACK(true), color: '#fff' } : {}}
      onClick={() => update({ ...s(), focus: i, area: 'rows', header: false, numbers: { tool, cell: 1 } })}
    >
      NUMBERS
      <Chevron dir="right" size={14} />
    </button>
  );
  return (
    <>
      <Show when={s().compare}>
        {(c) => (
          <>
            <div
              class="absolute inset-y-0 touch-none"
              style={{ left: `${PICTURE_LEFT}px`, right: '0' }}
              onPointerDown={(e) => e.currentTarget.setPointerCapture(e.pointerId)}
              onPointerMove={(e) => {
                if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
                const box = e.currentTarget.getBoundingClientRect();
                const x = PICTURE_LEFT + ((e.clientX - box.left) / box.width) * (W - PICTURE_LEFT);
                update({ ...s(), compare: { ...c(), split: Math.round(Math.max(PICTURE_LEFT + 30, Math.min(W - 30, x))) } });
              }}
            />
            <div class="pointer-events-none absolute inset-y-0 w-[3px] -translate-x-1/2 bg-white" style={{ left: `${c().split}px`, 'box-shadow': '0 0 6px rgb(0 0 0 / 0.6)' }} />
            <div class="pointer-events-none absolute top-[220px] flex size-[40px] -translate-x-1/2 items-center justify-center rounded-full bg-white text-[21px] font-semibold text-black" style={{ left: `${c().split}px` }}>
              ⇆
            </div>
            <div class="absolute" style={{ left: `${PICTURE_LEFT + 8}px`, top: `${s().crop ? stripMetrics(s().strip).band + 8 : 10}px` }}>
              <KeyHint glyph="A" label={s().name} />
            </div>
            <div class="absolute right-[10px]" data-anchor="compare" style={{ top: `${s().crop ? stripMetrics(s().strip).band + 8 : 10}px` }}>
              <KeyHint glyph="B" label={c().b === 'opened' ? 'AS OPENED' : BASES[c().b as number].name} tap={() => update(openPicker(s(), { kind: 'compare' }))} />
            </div>
          </>
        )}
      </Show>
      <Show when={s().crop > 0 && !s().picker}>
        <RawStrip s={s()} set={update} />
      </Show>

      <div
        class="absolute inset-y-0 left-0"
        style={{
          width: `${RAIL + SCROLLER}px`,
          ...props.column,
          ...(s().full || s().keyboard ? { opacity: 0, 'pointer-events': 'none' } : {}),
          transition: [props.column?.transition, 'opacity 200ms ease-out'].filter(Boolean).join(', '),
        }}
      >
      <div class="absolute inset-y-0 left-0 flex flex-col items-center gap-[4px] pt-[8px]" style={{ ...COLUMN, width: `${RAIL}px` }}>
        <For each={railSections(s())}>
          {(tool) => {
            const chosen = () => (s().area === 'rows' ? inView() : currentSection(s())) === tool;
            const lit = () => chosen() && s().area === 'rail';
            return (
              <button
                type="button"
                class="relative flex size-[52px] items-center justify-center rounded-[14px]"
                style={lit() ? SELECTED : chosen() ? { background: 'var(--m-focus)' } : {}}
                onClick={() => {
                  scrollToTool(tool, true);
                  const on = tool === 'film' || tool === 'raw' || tool === 'custom' ? tool : null;
                  update({ ...s(), focus: on ? s().focus : index(tool as Tool), railOn: on, area: 'rail', header: false, inside: false, bend: false, scrollTo: undefined });
                }}
              >
                <ToolIcon tool={tool} color="#fff" />
                <Show when={tool !== 'film' && tool !== 'raw' && tool !== 'custom' && edited(tool)}>
                  <span class="absolute top-[5px] right-[5px] size-[9px] rounded-full" style={{ background: ACCENT, 'box-shadow': '0 0 0 2px #1c1c1e' }} />
                </Show>
              </button>
            );
          }}
        </For>
      </div>

      <Show when={s().numbers}>
        <NumbersSheet s={s()} update={update} />
      </Show>
      <Scroller
        ref={(element) => (scroller = element)}
        class="absolute inset-y-0 px-[6px]"
        style={{ left: `${RAIL}px`, width: `${SCROLLER}px`, visibility: s().numbers ? 'hidden' : undefined }}
        onScroll={spy}
      >
        <Section ref={(element) => (sections.film = element)} first>
          <div class="mb-[8px] flex gap-[8px]">
            <button
              type="button"
              data-anchor="icon"
              class="flex size-[57px] shrink-0 items-center justify-center"
              style={TRACK(s().area === 'icon')}
              onClick={() => update(openPicker({ ...s(), area: 'icon', railOn: null, header: false, inside: false }, { kind: 'icon' }))}
            >
              <FilmIcon slug={s().icon} size={40} color="#fff" />
            </button>
            <div
              class="flex h-[57px] min-w-0 grow items-center gap-[10px] px-[10px]"
              style={TRACK(s().area === 'name')}
              onPointerDown={() => update({ ...s(), area: 'name', railOn: null, header: false, inside: false, keyboard: typing(s().name, 'name') })}
            >
              <div class="flex min-w-0 grow flex-col gap-[5px]">
                <span class={`truncate font-semibold ${TEXT}`}>{s().name}</span>
                <Show when={s().from}>
                  <span class={`text-white/60 ${DETAIL}`}>from {s().from}</span>
                </Show>
              </div>
              <svg width="24" height="24" viewBox="0 0 24 24" class="shrink-0 opacity-70">
                <path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" fill="none" stroke="#fff" stroke-width="2" stroke-linejoin="round" />
              </svg>
            </div>
          </div>
          <div class="flex gap-[8px]">
            <For each={[['save', 'SAVE'], ['saveas', 'SAVE AS NEW'], ['open', 'OPEN']] as ['save' | 'saveas' | 'open', string][]}>
              {([area, label]) => (
                <Button
                  label={label}
                  anchor={area}
                  class={area === 'saveas' ? 'grow' : 'grow basis-0'}
                  focused={s().area === area}
                  idle={area === 'save' && same(s().film, s().opened)}
                  onClick={() => {
                    const next = { ...s(), area, railOn: null, header: false, inside: false };
                    update(area === 'open' ? openPicker(next, { kind: 'open' }) : area === 'save' ? save(next) : { ...next, keyboard: typing(nextName(s().name), 'saveas') });
                  }}
                />
              )}
            </For>
          </div>
        </Section>
        <Show when={s().crop > 0}>
          <Section ref={(element) => (sections.raw = element)} title="RAW CONVERSION">
            <div
              class="mb-[8px] flex h-[57px] items-center gap-[10px] px-[10px]"
              style={TRACK(s().area === 'strip')}
              onPointerDown={() => update({ ...s(), area: 'strip', railOn: null, header: false, inside: false })}
            >
              <div class="flex grow flex-col gap-[5px]">
                <span class={`font-medium ${TEXT}`}>{RAWS[s().crop - 1].name}</span>
                <span class={`text-white/60 tabular-nums ${DETAIL}`}>{RAWS[s().crop - 1].info}</span>
              </div>
              <span class={`text-white/60 tabular-nums ${TEXT}`}>
                {s().crop}/{RAWS.length}
              </span>
            </div>
            <Button
              label="REFLECT SHOOTING COND."
              class="mb-[8px] w-full"
              focused={s().area === 'reflect'}
              onClick={() => update({ ...s(), area: 'reflect', railOn: null, header: false, inside: false, raw: RAW_DEFAULT, custom: true })}
            />
            <div class="mb-[8px] flex flex-col gap-[4px]">
              <For each={RAW_OPTIONS}>
                {(o, k) => (
                  <Choice
                    name={o.name}
                    items={o.values}
                    value={s().raw[k()]}
                    width={CONTENT}
                    focused={s().area === o.key}
                    onFocus={() => update({ ...s(), area: o.key, railOn: null, header: false, inside: false })}
                    pick={(v) => {
                      const raw = [...s().raw];
                      raw[k()] = v;
                      update({ ...s(), raw, area: o.key });
                    }}
                  />
                )}
              </For>
            </div>
            <Button
              label={s().created === s().crop ? 'CREATED' : 'CREATE'}
              class="w-full"
              focused={s().area === 'create'}
              accent={s().created === s().crop}
              onClick={() => update({ ...s(), area: 'create', railOn: null, header: false, inside: false, created: s().crop })}
            />
          </Section>
        </Show>
        <Section ref={(element) => (sections.custom = element)} title="PREVIEW WITH CUSTOM SETTING" long>
          <Switch
            items={['OFF', 'ON']}
            value={s().custom ? 1 : 0}
            focused={s().area === 'custom'}
            onFocus={() => update({ ...s(), area: 'custom', railOn: null, header: false, inside: false })}
            pick={(v) => update({ ...s(), custom: v === 1, area: 'custom' })}
          />
          <div class="mt-[10px] flex flex-wrap gap-x-[14px] gap-y-[8px] px-[6px]" style={{ opacity: s().custom ? 1 : 0.4 }}>
            <For each={RECIPE_ROWS.slice(1)}>
              {(row) => (
                <span class="flex items-center gap-[4px]">
                  <Artwork src={stockArtwork(row.icon)} class="size-[28px]" color="rgb(255 255 255 / 0.8)" />
                  <span class={`tabular-nums ${DETAIL}`}>{row.value ?? signed(row.level ?? 0)}</span>
                </span>
              )}
            </For>
          </div>
        </Section>
        <For each={TOOLS}>
          {(tool) => {
            const from = () => groupSource(s().film, tool);
            return (
              <Section
                ref={(element) => (sections[tool] = element)}
                title={TOOL_NAMES[tool]}
                long={tool === 'tone'}
                accessory={
                  <button
                    type="button"
                    data-anchor={`copy-${tool}`}
                    class={`flex h-[40px] shrink-0 items-center gap-[6px] rounded-[10px] pr-[8px] pl-[10px] whitespace-nowrap font-medium ${DETAIL}`}
                    style={{ background: 'var(--m-chip)', color: 'rgb(255 255 255 / 0.85)', 'box-shadow': headerFocused(tool) ? 'var(--m-focus-edge), 0 0 0 2px #fff' : 'var(--m-chip-edge)' }}
                    onClick={() => update(openPicker({ ...s(), focus: index(tool), header: true, area: 'rows' }, { kind: 'copy', tool }))}
                  >
                    <Show when={from() === undefined}>
                      <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
                    </Show>
                    {from() === undefined ? 'EDITED' : BASES[from() as number].name}
                    <Chevron dir="down" size={14} />
                  </button>
                }
              >
                <Show when={tool === 'curve'}>
                  <div ref={(element) => (rows[0] = element)} class="relative rounded-[14px] p-[2px]" style={frame(0)} onPointerDown={() => focusControl(0)}>
                    <button
                      type="button"
                      class="absolute top-[10px] left-[10px] z-10 flex size-[36px] items-center justify-center rounded-[10px]"
                      style={{ background: 'rgb(255 255 255 / 0.14)' }}
                      onPointerDown={(e) => e.stopPropagation()}
                      onClick={() => update({ ...s(), focus: 0, area: 'rows', header: false, inside: true, full: true })}
                    >
                      <ExpandIcon />
                    </button>
                    <CurveGraph
                      film={s().film}
                      opened={s().opened}
                      histogram={props.histogram}
                      width={CONTENT - 4}
                      height={220}
                      focus={s().inside && isFocused(0) ? s().knot : -1}
                      bend={s().inside && isFocused(0) && s().bend}
                      set={(film) => update({ ...s(), film })}
                      focusKnot={(knot) => update({ ...s(), knot, focus: 0, area: 'rows', header: false, inside: true })}
                    />
                    <div class={`h-[31px] px-[10px] pt-[2px] pb-[8px] font-medium tabular-nums ${SMALL}`}>{s().inside && isFocused(0) ? knotReadout(s().film, s().knot, s().bend) : ''}</div>
                  </div>
                </Show>
                <Show when={tool === 'color'}>
                  <ColorWheel film={s().film} set={(film) => update({ ...s(), film })} />
                  <For each={COLOR_SCALES}>{(c, k) => scaleRow(index('color', k()), c, CONTENT, 146)}</For>
                  {numbersRow(index('color', 3), 'color')}
                </Show>
                <Show when={tool === 'mix'}>
                  <Scope
                    film={s().film}
                    crop={s().crop}
                    chosen={s().primary}
                    choose={(primary) => update({ ...s(), primary, focus: index('mix', 1), area: 'rows', header: false })}
                    set={(film) => update({ ...s(), film })}
                  />
                  <div ref={(element) => (rows[index('mix')] = element)} class="my-[2px] flex gap-[6px] rounded-[12px] p-[5px]" style={TRACK(isFocused(index('mix')))} onPointerDown={() => focusControl(index('mix'))}>
                    <For each={MIX}>
                      {(mix, p) => (
                        <button
                          type="button"
                          class={`flex h-[40px] flex-1 items-center justify-center gap-[7px] rounded-[9px] font-medium ${TEXT}`}
                          style={s().primary === p() ? { ...SELECTED, color: '#fff' } : { color: 'rgb(255 255 255 / 0.75)' }}
                          onClick={() => update({ ...s(), primary: p() })}
                        >
                          <Swatch film={s().film} mix={p()} />
                          {mix.name}
                        </button>
                      )}
                    </For>
                  </div>
                  <For each={[0, 1]}>{(k) => scaleRow(index('mix', 1 + k), mixScales(s().primary)[k], CONTENT, 146)}</For>
                  {numbersRow(index('mix', 3), 'mix')}
                </Show>
                <Show when={tool === 'tone'}>
                  <div ref={(element) => (rows[index('tone')] = element)} class="rounded-[14px] p-[6px]" style={frame(index('tone'))} onPointerDown={() => focusControl(index('tone'))}>
                    <ToneGraph
                      film={s().film}
                      opened={s().opened}
                      width={CONTENT - 12}
                      height={150}
                      band={s().band}
                      inside={s().inside && isFocused(index('tone'))}
                      set={(film) => update({ ...s(), film })}
                      focusBand={(band) => update({ ...s(), band, focus: index('tone'), area: 'rows', header: false, inside: true })}
                    />
                    <div class={`h-[29px] px-[4px] pt-[8px] pb-[2px] font-medium tabular-nums ${SMALL}`}>
                      {s().inside && isFocused(index('tone')) ? `${BAND_NAMES[s().band]} · ${signed(s().film.bands[s().band])}%` : ''}
                    </div>
                  </div>
                </Show>
                <Show when={tool === 'channels'}>
                  <For each={CHANNEL_SCALES}>{(c, k) => scaleRow(index('channels', k()), c, CONTENT, 146)}</For>
                </Show>
              </Section>
            );
          }}
        </For>
        <div class="h-[300px]" />
      </Scroller>
      </div>

      <Show when={s().full}>
        <div class="absolute inset-0" style={{ background: 'rgb(0 0 0 / 0.38)' }}>
          <div class="absolute top-[14px] left-[24px]">
            <CurveGraph
              film={s().film}
              opened={s().opened}
              histogram={props.histogram}
              width={W - 48}
              height={H - 80}
              focus={s().knot}
              bend={s().bend}
              set={(film) => update({ ...s(), film })}
              focusKnot={(knot) => update({ ...s(), knot })}
            />
          </div>
          <button
            type="button"
            class="absolute top-[18px] left-[28px] flex size-[40px] items-center justify-center rounded-[10px]"
            style={GLASS}
            onClick={() => update({ ...s(), full: false })}
          >
            <ExpandIcon full />
          </button>
          <div class={`absolute bottom-[16px] left-[24px] flex h-[36px] items-center rounded-[12px] px-[12px] font-medium tabular-nums ${SMALL}`} style={GLASS}>
            {knotReadout(s().film, s().knot, s().bend)}
          </div>
        </div>
      </Show>

      <Show when={!s().picker && !s().keyboard}>
        <div class="absolute right-[10px] bottom-[10px] flex flex-col items-end gap-[6px]" style={props.chrome}>
          <Show when={s().inside && CONTROLS[s().focus].kind === 'curve'}>
            <KeyHint glyph="OK" label={s().full ? 'SIDEBAR' : 'FULL SCREEN'} tap={() => update(press(s(), 'ok'))} />
          </Show>
          {/* Full screen keeps the picture clear: only the way back shows. */}
          <Show when={!s().full}>
            <KeyHint glyph="Fn" label={s().custom ? 'CUSTOM ON' : 'CUSTOM OFF'} accent={!s().custom} tap={() => update(press(s(), 'fn'))} />
            <div class="flex gap-[6px]">
              <KeyHint glyph="Q" label="A|B" accent={!!s().compare} tap={() => update(press(s(), 'q'))} />
              <KeyHint glyph="▶" label={s().crop ? 'LIVE' : 'RAW'} tap={() => update(press(s(), 'play'))} />
            </div>
          </Show>
        </div>
      </Show>

      <Show when={s().keyboard}>{(t) => <Keyboard s={s()} t={t()} update={update} />}</Show>

      <Show when={s().picker}>
        {(picker) => {
          const p = () => picker();
          const options = () => pickerOptions(p());
          const grid = () => p().kind === 'icon';
          const anchor = () => {
            const q = p();
            return q.kind === 'copy' ? `copy-${q.tool}` : q.kind;
          };
          return (
            <AnchoredFlyOut
              anchor={anchor()}
              left={PICTURE_LEFT}
              height={grid() ? Math.ceil(options().length / ICON_COLUMNS) * (ICON_CELL + 4) + 2 : flyOutListHeight(options().length)}
            >
            <div style={grid() ? { width: `${ICON_COLUMNS * ICON_CELL}px` } : {}} class="flex min-h-0 grow flex-col">
            <PickerList
              options={options().map((k) =>
                p().kind === 'icon'
                  ? { label: String(k), art: (ink: string) => <FilmIcon slug={ICONS[k as number]} size={36} color={ink} /> }
                  : { label: pickerLabel(p(), k) },
              )}
              at={p().at}
              current={(() => {
                const c = pickerCurrent(s(), p());
                return c === undefined ? undefined : options().indexOf(c);
              })()}
              columns={p().kind === 'icon' ? ICON_COLUMNS : undefined}
              choose={(i) => update(choose(s(), p(), options()[i]))}
            />
            </div>
            </AnchoredFlyOut>
          );
        }}
      </Show>
    </>
  );
}

export function useHistogram() {
  const [histogram, setHistogram] = createSignal<number[]>([]);
  onSettled(() => {
    void source().then((src) => setHistogram(src.histogram));
  });
  return histogram;
}


function Screen(props: { state: () => State; set: (s: State) => void }) {
  const histogram = useHistogram();
  return (
    <>
      <Picture
        film={props.state().film}
        custom={props.state().custom}
        crop={props.state().crop}
        compare={(() => {
          const s = props.state();
          const b = compareFilm(s);
          return b && s.compare ? { film: b, split: s.compare.split } : undefined;
        })()}
      />
      <Editor s={props.state()} set={props.set} histogram={histogram()} />
    </>
  );
}

/* ---- Camera keys, on screen -------------------------------------------------------- */



/** The custom setting's rows, as the CUSTOM SETTING section summarizes them. */
type RecipeRow = { icon: number; value?: string; level?: number };
const RECIPE_ROWS: RecipeRow[] = [
  { icon: 0x5f3 },
  { icon: 0x609, value: 'Auto' },
  { icon: 0x5f1, value: 'DR100' },
  { icon: 0x5f6, level: 1 },
  { icon: 0x607, level: -1 },
  { icon: 0x5f0, level: 0 },
  { icon: 0x605, level: 0 },
  { icon: 0x603, level: -2 },
];




/* ---- Review ------------------------------------------------------------------------- */

export const FILM_STATES: { id: string; title: string; state: State }[] = [
  { id: 'film', title: 'The film: name, save, save as new, open', state: initial({ area: 'save' }) },
  { id: 'save-as', title: 'Save as new: name the copy', state: initial({ area: 'saveas', keyboard: { ...typing(nextName('NEW FILM'), 'saveas'), focus: 4 } }) },
  { id: 'raw-options', title: 'RAW conversion options', state: initial({ crop: 3, area: 'push', raw: [6, 1, 0, 0, 0, 0], scrollTo: 'raw' }) },
  { id: 'icon', title: 'Pick the film\'s icon', state: openPicker(initial({ area: 'icon', icon: 'custom' }), { kind: 'icon' }) },
  { id: 'name', title: 'Type the film\'s name over live view', state: initial({ area: 'name', keyboard: { ...typing('Warm Fa', 'name'), focus: 18 } }) },
  { id: 'monogram', title: 'Letter the icon yourself', state: initial({ area: 'icon', keyboard: { ...typing('Wf', 'icon'), shift: 'off', focus: 4 } }) },
  { id: 'open', title: 'Open another film', state: openPicker(initial({ area: 'open' }), { kind: 'open' }) },
  { id: 'curve', title: 'Inside the tone curve', state: initial({ focus: 0, knot: 7, inside: true, scrollTo: 'curve' }) },
  { id: 'full', title: 'The tone curve full screen', state: initial({ focus: 0, knot: 7, inside: true, full: true }) },
  { id: 'color', title: 'Color: the wheel', state: initial({ focus: 1, scrollTo: 'color' }) },
  { id: 'color-mix', title: 'Color mix: the vectorscope', state: initial({ focus: 6, scrollTo: 'mix' }) },
  { id: 'mix-numbers', title: 'Color mix as numbers', state: initial({ focus: 8, scrollTo: 'mix', numbers: { tool: 'mix', cell: 1 } }) },
  { id: 'color-numbers', title: 'Color as numbers', state: initial({ focus: 4, scrollTo: 'color', numbers: { tool: 'color', cell: 1 } }) },
  { id: 'tone', title: 'Saturation by tone', state: initial({ focus: CONTROLS.findIndex((c) => c.kind === 'tone'), band: 4, inside: true, scrollTo: 'tone' }) },
  { id: 'copy', title: 'Copy one tool from a film', state: openPicker(initial({ header: true }), { kind: 'copy', tool: 'curve' }) },
  { id: 'compare', title: 'Compare with a film', state: initial({ compare: { b: 0, split: SPLIT } }) },
  { id: 'film-only', title: 'Custom setting off: the film alone', state: initial({ custom: false, area: 'custom', scrollTo: 'custom' }) },
  ...STRIP_STYLES.map(([strip, name]) => ({ id: `raw-${strip}`, title: `RAW files: ${name.toLowerCase()}`, state: initial({ crop: 3, area: 'strip', strip }) })),
];

/** A review state, live: its keys still work. */
export function FilmScreen(props: { start: State }) {
  const [state, setState] = createSignal<State>(props.start);
  return <Screen state={state} set={setState} />;
}

