import { render } from '@solidjs/web';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import './focal.css';

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

function baseFilm(base: number): Film {
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

/* ---- Developing the picture ----------------------------------------------------- */

const PHOTO = new URL('../../engine/assets/default-scene.png', import.meta.url).href;
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
      const histogram = new Array<number>(64).fill(0);
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
  const m = primariesMatrix(f.primaries);
  const gr = 2 ** (f.red / 10);
  const gb = 2 ** (f.blue / 10);
  const s = 1 + f.saturation / 100;
  const a = (f.hue * Math.PI) / 180;
  /* Warm/cool stretches chroma along the orange-blue axis. */
  const dx = -0.6;
  const dy = 0.8;
  const w = f.warmth / 100;
  const k00 = 1 + w * dx * dx;
  const k01 = w * dx * dy;
  const k11 = 1 + w * dy * dy;
  const c = Math.cos(a) * s;
  const sn = Math.sin(a) * s;
  const a00 = c * k00 - sn * k01;
  const a01 = c * k01 - sn * k11;
  const a10 = sn * k00 + c * k01;
  const a11 = sn * k01 + c * k11;
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
function Picture(props: { film: Film; custom: boolean; crop: number; compare?: { film: Film; split: number } }) {
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

const ACCENT = '#7d3fc2';
const TEXT = 'text-[24px] leading-none';
const SMALL = 'text-[21px] leading-none';
const GLASS: JSX.CSSProperties = {
  background: 'rgb(22 22 24 / 0.62)',
  'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.28), inset 0 0 0 1px rgb(255 255 255 / 0.1)',
  color: '#fff',
};
const COLUMN: JSX.CSSProperties = {
  background: 'rgb(22 22 24 / 0.6)',
  'box-shadow': 'inset -1px 0 0 rgb(255 255 255 / 0.22)',
  color: '#fff',
};
const TRACK = (focused: boolean): JSX.CSSProperties => ({
  background: focused ? 'rgb(255 255 255 / 0.17)' : 'rgb(255 255 255 / 0.06)',
  'border-radius': '13px',
  'box-shadow': focused ? 'inset 0 1px 0 rgb(255 255 255 / 0.35), inset 0 0 0 1px rgb(255 255 255 / 0.14)' : undefined,
});
const BAND_NAMES = ['BLACKS', 'SHADOWS', 'MIDTONES', 'HIGHLIGHTS', 'WHITES'];

const signed = (n: number) => (n > 0 ? `+${n}` : n < 0 ? `−${-n}` : '0');

/* ---- Controls --------------------------------------------------------------------- */

function Ruler(props: {
  value: number;
  min: number;
  max: number;
  step: number;
  width: number;
  focused: boolean;
  changed: boolean;
  set: (value: number) => void;
  /** A gradient under the ticks showing what the scale does. */
  track?: string;
}) {
  const spacing = 34;
  const index = () => (props.value - props.min) / props.step;
  const count = () => Math.round((props.max - props.min) / props.step) + 1;
  const zero = () => (0 - props.min) / props.step;
  const origin = () => props.width / 2 - index() * spacing;
  const color = () => (props.changed ? ACCENT : '#fff');
  let drag: { x: number; value: number } | undefined;
  return (
    <div
      class="relative h-full overflow-hidden"
      style={{
        width: `${props.width}px`,
        'touch-action': 'pan-y',
        'mask-image': 'linear-gradient(to right, transparent, #000 36px, #000 calc(100% - 36px), transparent)',
      }}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        drag = { x: e.clientX, value: props.value };
      }}
      onPointerMove={(e) => {
        if (!drag) return;
        const box = e.currentTarget.getBoundingClientRect();
        const scale = e.currentTarget.offsetWidth / box.width;
        const steps = Math.round(((drag.x - e.clientX) * scale) / spacing);
        props.set(Math.max(props.min, Math.min(props.max, drag.value + steps * props.step)));
      }}
      onPointerUp={() => (drag = undefined)}
      onPointerCancel={() => (drag = undefined)}
    >
      <Show when={props.track}>
        {(track) => <div class="absolute inset-x-0 bottom-[5px] h-[9px] rounded-full opacity-80" style={{ background: track() }} />}
      </Show>
      <div class="absolute inset-y-0 left-0 transition-transform duration-150 ease-out" style={{ transform: `translateX(${origin()}px)` }}>
        <For each={Array.from({ length: count() }, (_, i) => i)}>
          {(i) => {
            const lit = () => (i >= Math.min(zero(), index()) && i <= Math.max(zero(), index()) && index() !== zero());
            return (
              <span
                class="absolute bottom-[5px] -translate-x-1/2"
                style={{
                  left: `${i * spacing}px`,
                  width: '2px',
                  height: `${i === zero() ? 14 : 9}px`,
                  background: lit() ? ACCENT : 'rgb(255 255 255 / 0.55)',
                }}
              />
            );
          }}
        </For>
      </div>
      <span
        class={`absolute top-[3px] -translate-x-1/2 font-semibold tabular-nums ${SMALL}`}
        style={{ left: `${props.width / 2}px`, color: color() }}
      >
        {signed(props.value)}
      </span>
      <span class="absolute bottom-[4px] h-[17px] w-[3px] -translate-x-1/2 rounded-full" style={{ left: `${props.width / 2}px`, background: color() }} />
    </div>
  );
}

/** A labeled row: name on the left, the control in its track on the right. */
function Row(props: { name: string; focused: boolean; width: number; nameWidth?: number; children: JSX.Element; onClick?: () => void }) {
  const nameWidth = () => props.nameWidth ?? 146;
  return (
    <div class="flex h-[57px] items-center gap-[8px]" style={{ width: `${props.width}px` }} onClick={() => props.onClick?.()}>
      <span
        class={`shrink-0 font-light tracking-[0.06em] ${SMALL}`}
        style={{ width: `${nameWidth()}px`, color: props.focused ? '#fff' : 'rgb(255 255 255 / 0.72)' }}
      >
        {props.name}
      </span>
      <div class="h-[47px] grow overflow-hidden" style={TRACK(props.focused)}>
        {props.children}
      </div>
    </div>
  );
}

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
type Section = 'raw' | 'custom' | Tool;
const TOOL_NAMES: Record<Tool, string> = {
  curve: 'TONE CURVE',
  color: 'COLOR',
  mix: 'COLOR MIX',
  tone: 'SATURATION BY TONE',
  channels: 'CHANNELS',
};

const GROUPS: Record<Tool, (f: Film) => unknown> = {
  curve: (f) => [f.knots, f.tilt],
  color: (f) => [f.saturation, f.hue, f.warmth],
  mix: (f) => f.primaries,
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
      return { ...f, saturation: b.saturation, hue: b.hue, warmth: b.warmth };
    case 'mix':
      return { ...f, primaries: b.primaries };
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
  { name: 'SATURATION', get: (f) => f.saturation, put: (f, v) => ({ ...f, saturation: v }), min: -50, max: 50, step: 5, track: gradient(GRAY, '#ff9500') },
  { name: 'HUE', get: (f) => f.hue, put: (f, v) => ({ ...f, hue: v }), min: -15, max: 15, step: 1, track: gradient('#ff375f', '#ff9f0a', '#ffd60a', '#30d158', '#0a84ff', '#bf5af2') },
  { name: 'WARM/COOL', get: (f) => f.warmth, put: (f, v) => ({ ...f, warmth: v }), min: -50, max: 50, step: 5, track: gradient('#0a84ff', GRAY, '#ff9f0a') },
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
    put: (f, v) => ({ ...f, primaries: f.primaries.map((q, i) => (i === p ? { ...q, hue: v } : q)) }),
    min: -30,
    max: 30,
    step: 2,
    track: MIX[p].hue,
  },
  {
    name: 'SATURATION',
    get: (f) => f.primaries[p].sat,
    put: (f, v) => ({ ...f, primaries: f.primaries.map((q, i) => (i === p ? { ...q, sat: v } : q)) }),
    min: -50,
    max: 50,
    step: 5,
    track: gradient(GRAY, MIX[p].color),
  },
];

const CHANNEL_SCALES: Scale[] = [
  { name: 'RED CURVE', get: (f) => f.red, put: (f, v) => ({ ...f, red: v }), min: -5, max: 5, step: 1, track: gradient('#00c7be', GRAY, '#ff3b30') },
  { name: 'BLUE CURVE', get: (f) => f.blue, put: (f, v) => ({ ...f, blue: v }), min: -5, max: 5, step: 1, track: gradient('#ffd60a', GRAY, '#0a84ff') },
];

type Control = { tool: Tool; kind: 'curve' } | { tool: Tool; kind: 'tone' } | { tool: Tool; kind: 'scale'; scale: Scale; mix?: number };

const CONTROLS: Control[] = [
  { tool: 'curve', kind: 'curve' },
  ...COLOR_SCALES.map((scale): Control => ({ tool: 'color', kind: 'scale', scale })),
  ...[0, 1, 2].flatMap((p) => mixScales(p).map((scale): Control => ({ tool: 'mix', kind: 'scale', scale, mix: p }))),
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

interface State {
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
  picker: Picker | null;
  /** Where the D-pad is: the RAW strip, the tool rail or the tool rows. */
  area: 'strip' | 'create' | 'custom' | 'rail' | 'rows';
  /** On the rail, the RAW or CUSTOM SETTING section instead of a tool. */
  railOn: 'raw' | 'custom' | null;
  strip: StripStyle;
  /** The RAW last developed with OK. */
  created: number;
  /** Review states open scrolled to a tool. */
  scrollTo?: Section;
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

type Picker = { kind: 'copy'; tool: Tool; at: number } | { kind: 'compare'; at: number };

function pickerOptions(p: Picker): (number | 'opened')[] {
  return p.kind === 'compare' ? ['opened', ...BASES.map((_, i) => i)] : BASES.map((_, i) => i);
}

function choose(s: State, p: Picker, k: number | 'opened'): State {
  if (p.kind === 'copy') return k === 'opened' ? s : { ...s, picker: null, film: copyGroup(s.film, p.tool, k) };
  return { ...s, picker: null, compare: { b: k, split: s.compare?.split ?? SPLIT } };
}

function openPicker(s: State, p: { kind: 'copy'; tool: Tool } | { kind: 'compare' }): State {
  const current = p.kind === 'copy' ? groupSource(s.film, p.tool) : s.compare?.b;
  const at = Math.max(0, pickerOptions({ ...p, at: 0 }).indexOf(current ?? -1));
  return { ...s, picker: { ...p, at } };
}

function initial(over: Partial<State>): State {
  return { film: EDITED, opened: START, focus: 0, knot: 4, bend: false, band: 1, inside: false, header: false, custom: true, compare: null, crop: 0, picker: null, area: 'rows', railOn: null, strip: 'frames', created: 0, ...over };
}

const RAIL = 60;
const SCROLLER = 352;
const CONTENT = SCROLLER - 12;
const PICTURE_LEFT = RAIL + SCROLLER;
const SPLIT = Math.round((PICTURE_LEFT + W) / 2);

function compareFilm(s: State): Film | undefined {
  if (!s.compare) return undefined;
  return s.compare.b === 'opened' ? s.opened : baseFilm(s.compare.b);
}

type Key = 'left' | 'right' | 'up' | 'down' | 'ok' | 'back' | 'dial-' | 'dial+' | 'q' | 'fn' | 'play';

const firstOf = (tool: Tool) => CONTROLS.findIndex((c) => c.tool === tool);

function railSections(s: State): Section[] {
  return [...(s.crop ? (['raw'] as Section[]) : []), 'custom', ...TOOLS];
}

/** The section the focus is in, or on the rail, the section chosen there. */
function currentSection(s: State): Section {
  switch (s.area) {
    case 'strip':
    case 'create':
      return 'raw';
    case 'custom':
      return 'custom';
    case 'rail':
      return s.railOn ?? CONTROLS[s.focus].tool;
    case 'rows':
      return CONTROLS[s.focus].tool;
  }
}

function enterSection(s: State, section: Section): State {
  if (section === 'raw') return { ...s, area: 'strip', railOn: null };
  if (section === 'custom') return { ...s, area: 'custom', railOn: null };
  return { ...s, area: 'rows', railOn: null, focus: firstOf(section), header: false };
}

function press(s: State, key: Key): State {
  const control = CONTROLS[s.focus];
  const f = s.film;
  const d = key === 'left' || key === 'down' || key === 'dial-' ? -1 : 1;
  if (s.picker) {
    const p = s.picker;
    const options = pickerOptions(p);
    switch (key) {
      case 'up':
      case 'dial-':
        return { ...s, picker: { ...p, at: Math.max(0, p.at - 1) } };
      case 'down':
      case 'dial+':
        return { ...s, picker: { ...p, at: Math.min(options.length - 1, p.at + 1) } };
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
  switch (s.area) {
    case 'strip':
      switch (key) {
        case 'down':
        case 'ok':
          return { ...s, area: 'create' };
        case 'back':
          return { ...s, area: 'rail', railOn: 'raw' };
        case 'up':
          return s;
      }
      return { ...s, crop: Math.max(1, Math.min(RAWS.length, s.crop + step)), created: 0 };
    case 'create':
      switch (key) {
        case 'up':
          return { ...s, area: 'strip' };
        case 'down':
          return { ...s, area: 'custom' };
        case 'ok':
          return { ...s, created: s.crop };
        case 'back':
          return { ...s, area: 'rail', railOn: 'raw' };
      }
      return s;
    case 'custom':
      switch (key) {
        case 'up':
          return s.crop ? { ...s, area: 'create' } : s;
        case 'down':
          return { ...s, area: 'rows', focus: 0, header: true };
        case 'back':
          return { ...s, area: 'rail', railOn: 'custom' };
      }
      return { ...s, custom: !s.custom };
    case 'rail': {
      const list = railSections(s);
      const at = list.indexOf(currentSection(s));
      switch (key) {
        case 'up':
        case 'down':
        case 'dial-':
        case 'dial+': {
          const next = list[at + (key === 'up' || key === 'dial-' ? -1 : 1)];
          if (!next) return s;
          if (next === 'raw' || next === 'custom') return { ...s, railOn: next, scrollTo: undefined };
          return { ...s, railOn: null, focus: firstOf(next), header: false, scrollTo: undefined };
        }
        case 'right':
        case 'ok':
          return enterSection(s, currentSection(s));
      }
      return s;
    }
  }
  if (s.inside) {
    if (key === 'ok' || key === 'back') return { ...s, inside: false, bend: false };
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
      return control.kind === 'scale' ? s : { ...s, inside: true };
  }
  if (s.header || control.kind !== 'scale') return s;
  const c = control.scale;
  return { ...s, film: c.put(f, Math.max(c.min, Math.min(c.max, c.get(f) + step * c.step))) };
}

function ToolIcon(props: { tool: Section; color: string }) {
  const stroke = () => ({ fill: 'none', stroke: props.color, 'stroke-width': 2.5, 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const });
  return (
    <svg width="32" height="32" viewBox="0 0 34 34">
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
    const m = primariesMatrix(props.film.primaries);
    const l = MIX[props.mix].linear;
    const rgb = [0, 1, 2].map((r) => encode(Math.max(0, Math.min(1, m[r * 3] * l[0] + m[r * 3 + 1] * l[1] + m[r * 3 + 2] * l[2]))) * 255);
    return `rgb(${rgb.map(Math.round).join(' ')})`;
  };
  return (
    <span
      class="block size-[36px] shrink-0 rounded-full"
      style={{ background: `linear-gradient(90deg, ${MIX[props.mix].color} 50%, ${after()} 50%)`, 'box-shadow': 'inset 0 0 0 2px rgb(255 255 255 / 0.35)' }}
    />
  );
}

function Hint(props: { glyph: string; label?: string; tap?: () => void; accent?: boolean }) {
  return (
    <button
      type="button"
      class="flex h-[36px] items-center gap-[8px] rounded-[12px] px-[7px] whitespace-nowrap"
      style={{ ...GLASS, ...(props.accent ? { background: ACCENT } : {}) }}
      onClick={(e) => {
        e.stopPropagation();
        props.tap?.();
      }}
    >
      <span class="flex h-[24px] min-w-[24px] items-center justify-center rounded-[6px] bg-white px-[4px] text-[18px] leading-none font-semibold text-black">{props.glyph}</span>
      <Show when={props.label}>
        <span class={`pr-[3px] font-light tracking-[0.04em] ${SMALL}`}>{props.label}</span>
      </Show>
    </button>
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
                  <span class="text-[15px] leading-none font-medium tabular-nums" style={{ color: chosen() ? '#fff' : 'rgb(255 255 255 / 0.5)' }}>
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
function Editor(props: { s: State; set: (s: State) => void; histogram: number[]; title: { name: string; from?: string } }) {
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
  });
  createEffect(
    () => s().focus,
    (focus) => {
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
  const scaleRow = (i: number, c: Scale, width: number, nameWidth: number) => (
    <div ref={(element) => (rows[i] = element)} onPointerDown={() => focusControl(i)}>
      <Row name={c.name} focused={isFocused(i)} width={width} nameWidth={nameWidth}>
        <Ruler
          value={c.get(s().film)}
          min={c.min}
          max={c.max}
          step={c.step}
          width={width - nameWidth - 8}
          focused={isFocused(i)}
          changed={c.get(s().film) !== c.get(s().opened)}
          track={c.track}
          set={(v) => update({ ...s(), focus: i, film: c.put(s().film, v) })}
        />
      </Row>
    </div>
  );
  const index = (tool: Tool, nth = 0) => CONTROLS.findIndex((c) => c.tool === tool) + nth;
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
              <Hint glyph="A" label={props.title.name} />
            </div>
            <div class="absolute right-[10px]" style={{ top: `${s().crop ? stripMetrics(s().strip).band + 8 : 10}px` }}>
              <Hint glyph="B" label={c().b === 'opened' ? 'AS OPENED' : BASES[c().b as number].name} tap={() => update(openPicker(s(), { kind: 'compare' }))} />
            </div>
          </>
        )}
      </Show>
      <Show when={s().crop > 0 && !s().picker}>
        <RawStrip s={s()} set={update} />
      </Show>

      <div class="absolute inset-y-0 left-0 flex flex-col items-center gap-[4px] pt-[8px]" style={{ ...COLUMN, width: `${RAIL}px`, background: 'rgb(18 18 20 / 0.82)' }}>
        <For each={railSections(s())}>
          {(tool) => {
            const chosen = () => (s().area === 'rows' ? inView() : currentSection(s())) === tool;
            const lit = () => chosen() && s().area === 'rail';
            return (
              <button
                type="button"
                class="relative flex size-[52px] items-center justify-center rounded-[14px]"
                style={lit() ? { background: '#fff' } : chosen() ? { background: 'rgb(255 255 255 / 0.2)' } : {}}
                onClick={() => {
                  scrollToTool(tool, true);
                  const on = tool === 'raw' || tool === 'custom' ? tool : null;
                  update({ ...s(), focus: on ? s().focus : index(tool as Tool), railOn: on, area: 'rail', header: false, inside: false, bend: false, scrollTo: undefined });
                }}
              >
                <ToolIcon tool={tool} color={lit() ? '#000' : '#fff'} />
                <Show when={tool !== 'raw' && tool !== 'custom' && edited(tool)}>
                  <span class="absolute top-[5px] right-[5px] size-[9px] rounded-full" style={{ background: ACCENT, 'box-shadow': '0 0 0 2px #1c1c1e' }} />
                </Show>
              </button>
            );
          }}
        </For>
      </div>

      <div
        ref={(element) => (scroller = element)}
        class="absolute inset-y-0 overflow-y-auto overscroll-contain px-[6px]"
        style={{ ...COLUMN, left: `${RAIL}px`, width: `${SCROLLER}px`, 'scrollbar-width': 'none' }}
        onScroll={spy}
      >
        <div class="flex h-[54px] items-baseline gap-[10px] px-[6px] pt-[16px]">
          <span class={`font-semibold ${TEXT}`}>{props.title.name}</span>
          <Show when={props.title.from}>
            <span class={`text-white/60 ${SMALL}`}>{props.title.from}</span>
          </Show>
        </div>
        <Show when={s().crop > 0}>
          <section ref={(element) => (sections.raw = element)} class="pt-[10px] pb-[14px]" style={{ 'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.16)' }}>
            <div class={`mb-[6px] flex h-[44px] items-center px-[6px] font-semibold tracking-[0.02em] ${TEXT}`}>RAW FILE</div>
            <div
              class="mb-[8px] flex h-[57px] items-center gap-[10px] px-[10px]"
              style={TRACK(s().area === 'strip')}
              onPointerDown={() => update({ ...s(), area: 'strip', railOn: null, header: false, inside: false })}
            >
              <div class="flex grow flex-col gap-[5px]">
                <span class={`font-medium ${SMALL}`}>{RAWS[s().crop - 1].name}</span>
                <span class="text-[16px] leading-none text-white/60 tabular-nums">{RAWS[s().crop - 1].info}</span>
              </div>
              <span class={`text-white/60 tabular-nums ${SMALL}`}>
                {s().crop}/{RAWS.length}
              </span>
            </div>
            <button
              type="button"
              class={`flex h-[47px] w-full items-center justify-center rounded-[12px] font-semibold ${SMALL}`}
              style={{
                ...TRACK(s().area === 'create'),
                ...(s().created === s().crop ? { background: ACCENT } : {}),
              }}
              onClick={() => update({ ...s(), area: 'create', railOn: null, header: false, inside: false, created: s().crop })}
            >
              {s().created === s().crop ? 'CREATED' : 'CREATE'}
            </button>
          </section>
        </Show>
        <section ref={(element) => (sections.custom = element)} class="pt-[10px] pb-[14px]" style={{ 'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.16)' }}>
          <div class={`mb-[6px] flex h-[44px] items-center px-[6px] font-semibold tracking-[0.02em] ${TEXT}`}>CUSTOM SETTING</div>
          <div
            class="flex h-[47px] items-center gap-[4px] px-[5px]"
            style={TRACK(s().area === 'custom')}
            onPointerDown={() => update({ ...s(), area: 'custom', railOn: null, header: false, inside: false })}
          >
            <For each={[false, true]}>
              {(on) => (
                <button
                  type="button"
                  class={`flex h-[37px] grow items-center justify-center rounded-[10px] font-medium ${SMALL}`}
                  style={s().custom === on ? { background: '#fff', color: '#000' } : { color: 'rgb(255 255 255 / 0.6)' }}
                  onClick={() => update({ ...s(), custom: on, area: 'custom' })}
                >
                  {on ? 'ON' : 'OFF'}
                </button>
              )}
            </For>
          </div>
          <div class="mt-[10px] flex flex-wrap gap-x-[14px] gap-y-[8px] px-[6px]" style={{ opacity: s().custom ? 1 : 0.4 }}>
            <For each={RECIPE_ROWS.slice(1)}>
              {(row) => (
                <span class="flex items-center gap-[4px]">
                  <StockIcon id={row.icon} color="rgb(255 255 255 / 0.8)" size={28} />
                  <span class="text-[18px] leading-none tabular-nums">{row.value ?? signed(row.level ?? 0)}</span>
                </span>
              )}
            </For>
          </div>
        </section>
        <For each={TOOLS}>
          {(tool) => {
            const from = () => groupSource(s().film, tool);
            return (
              <section ref={(element) => (sections[tool] = element)} class="pt-[10px] pb-[14px]" style={{ 'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.16)' }}>
                <div class="mb-[6px] flex h-[44px] items-center gap-[8px] px-[6px]">
                  <span class={`grow font-semibold tracking-[0.02em] ${TEXT} ${tool === 'tone' ? '' : 'whitespace-nowrap'}`} style={{ 'font-size': tool === 'tone' ? '21px' : undefined }}>
                    {TOOL_NAMES[tool]}
                  </span>
                  <button
                    type="button"
                    class={`flex h-[36px] shrink-0 items-center gap-[4px] rounded-[10px] px-[8px] whitespace-nowrap ${SMALL}`}
                    style={{
                      ...(from() === undefined ? { background: ACCENT, color: '#fff' } : { background: 'rgb(255 255 255 / 0.14)', color: '#fff' }),
                      'box-shadow': headerFocused(tool) ? '0 0 0 3px #fff' : undefined,
                    }}
                    onClick={() => update(openPicker({ ...s(), focus: index(tool), header: true, area: 'rows' }, { kind: 'copy', tool }))}
                  >
                    {from() === undefined ? 'EDITED' : BASES[from() as number].name}
                    <span class="text-[15px]">▾</span>
                  </button>
                </div>
                <Show when={tool === 'curve'}>
                  <div ref={(element) => (rows[0] = element)} class="rounded-[14px] p-[2px]" style={frame(0)} onPointerDown={() => focusControl(0)}>
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
                  <For each={COLOR_SCALES}>{(c, k) => scaleRow(index('color', k()), c, CONTENT, 146)}</For>
                </Show>
                <Show when={tool === 'mix'}>
                  <For each={MIX}>
                    {(mix, p) => (
                      <div class="flex items-center gap-[8px]">
                        <div class="flex w-[48px] shrink-0 flex-col items-center gap-[4px]">
                          <Swatch film={s().film} mix={p()} />
                          <span class="text-[14px] font-medium tracking-[0.04em] text-white/70">{mix.name}</span>
                        </div>
                        <div>
                          <For each={mixScales(p())}>{(c, k) => scaleRow(index('mix', p() * 2 + k()), c, CONTENT - 56, 134)}</For>
                        </div>
                      </div>
                    )}
                  </For>
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
              </section>
            );
          }}
        </For>
        <div class="h-[300px]" />
      </div>

      <Show when={!s().picker}>
        <div class="absolute right-[10px] bottom-[10px] flex flex-col items-end gap-[6px]">
          <Hint glyph="Fn" label={s().custom ? 'CUSTOM ON' : 'CUSTOM OFF'} accent={!s().custom} tap={() => update(press(s(), 'fn'))} />
          <div class="flex gap-[6px]">
            <Hint glyph="Q" label="A|B" accent={!!s().compare} tap={() => update(press(s(), 'q'))} />
            <Hint glyph="▶" label={s().crop ? 'LIVE' : 'RAW'} tap={() => update(press(s(), 'play'))} />
          </div>
        </div>
      </Show>

      <Show when={s().picker}>
        {(picker) => {
          const current = () => {
            const p = picker();
            return p.kind === 'copy' ? groupSource(s().film, p.tool) : s().compare?.b;
          };
          const options = () => pickerOptions(picker());
          return (
            <div class="absolute top-[8px] bottom-[8px] flex flex-col overflow-hidden rounded-[16px] p-[8px]" style={{ ...GLASS, background: 'rgb(30 30 32 / 0.9)', left: `${PICTURE_LEFT + 8}px`, right: '8px' }}>
              <div class={`px-[8px] pt-[6px] pb-[10px] font-semibold ${SMALL}`}>
                {(() => {
                  const p = picker();
                  return p.kind === 'copy' ? `COPY ${TOOL_NAMES[p.tool]} FROM` : 'COMPARE WITH';
                })()}
              </div>
              <div class="grow overflow-y-auto" style={{ 'scrollbar-width': 'none' }}>
                <For each={options()}>
                  {(k, i) => (
                    <button
                      type="button"
                      class={`flex h-[44px] w-full items-center justify-between rounded-[10px] px-[10px] text-left ${SMALL}`}
                      style={picker().at === i() ? { background: '#fff', color: '#000' } : { color: '#fff' }}
                      onClick={() => update(choose(s(), picker(), k))}
                    >
                      {k === 'opened' ? 'AS OPENED' : BASES[k].name}
                      <Show when={current() === k}>
                        <span>✓</span>
                      </Show>
                    </button>
                  )}
                </For>
              </div>
            </div>
          );
        }}
      </Show>
    </>
  );
}

function useHistogram() {
  const [histogram, setHistogram] = createSignal<number[]>([]);
  onSettled(() => {
    void source().then((src) => setHistogram(src.histogram));
  });
  return histogram;
}

const NEW_TITLE = { name: 'NEW FILM', from: 'from Classic Chrome' };

function Screen(props: { state: () => State; set: (s: State) => void }) {
  const histogram = useHistogram();
  return (
    <div class="relative h-[480px] w-[720px] overflow-hidden bg-black font-sans select-none">
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
      <Editor s={props.state()} set={props.set} histogram={histogram()} title={NEW_TITLE} />
    </div>
  );
}

/* ---- Camera keys, on screen -------------------------------------------------------- */

function Pad(props: { send: (key: Key) => void }) {
  const button = (key: Key, label: string, extra = '') => (
    <button
      type="button"
      class={`flex h-[52px] min-w-[52px] items-center justify-center rounded-[14px] bg-[#2c2c2e] px-[10px] text-[17px] font-semibold text-white active:bg-[#48484a] ${extra}`}
      onClick={() => props.send(key)}
    >
      {label}
    </button>
  );
  return (
    <div class="mt-[12px] flex max-w-[720px] flex-wrap items-center justify-between gap-[14px]">
      <div class="grid grid-cols-3 gap-[6px]">
        <span />
        {button('up', '▲')}
        <span />
        {button('left', '◀')}
        {button('ok', 'OK')}
        {button('right', '▶')}
        <span />
        {button('down', '▼')}
        <span />
      </div>
      <div class="grid grid-cols-2 gap-[6px]">
        {button('dial-', 'DIAL −')}
        {button('dial+', 'DIAL +')}
        {button('q', 'Q')}
        {button('fn', 'Fn')}
        {button('play', 'PLAY ▶')}
        {button('back', 'BACK')}
      </div>
    </div>
  );
}

function Live(props: { start: State }) {
  const [state, setState] = createSignal<State>(props.start);
  return (
    <>
      <Fit>
        <Screen state={state} set={setState} />
      </Fit>
      <Pad send={(key) => setState(press(state(), key))} />
    </>
  );
}

/* ---- From the recipe editor --------------------------------------------------------- */

/* The recipe editor's FILM SIMULATION row opens the film editor. A built-in
   film opens as a new film copied from it; a card film opens as itself. Two
   presentations: a sheet rising from the bottom while the recipe column
   recedes, or the film editor sliding in over it from the left edge. */

const STOCK = '/review/stock-icons/x70-110';
const stockIcon = (id: number) => `${STOCK}/ff${id.toString(16).padStart(6, '0')}.svg`;
const RECIPE_WIDTH = 280;

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

function StockIcon(props: { id: number; color: string; size?: number }) {
  return (
    <span
      class="block shrink-0"
      style={{
        width: `${props.size ?? 38}px`,
        height: `${props.size ?? 38}px`,
        background: props.color,
        'mask-image': `url(${stockIcon(props.id)})`,
        'mask-size': 'contain',
        'mask-repeat': 'no-repeat',
        'mask-position': 'center',
      }}
    />
  );
}

function RecipeColumn(props: { film: string }) {
  return (
    <div class="absolute inset-y-0 left-0 overflow-hidden pt-[4px]" style={{ ...COLUMN, width: `${RECIPE_WIDTH}px` }}>
      <For each={RECIPE_ROWS}>
        {(row, i) => (
          <div class="flex h-[57px] items-center pr-[6px] pl-[10px]">
            <StockIcon id={row.icon} color={i() === 0 ? '#fff' : 'rgb(255 255 255 / 0.72)'} />
            <div class="ml-[8px] flex h-[47px] grow items-center justify-center" style={TRACK(i() === 0)}>
              <Show
                when={row.level === undefined}
                fallback={<Ruler value={row.level ?? 0} min={-2} max={2} step={1} width={208} focused={false} changed={false} set={() => undefined} />}
              >
                <span class={`flex h-[37px] items-center rounded-[10px] bg-white px-[12px] font-medium whitespace-nowrap text-black ${SMALL}`}>{i() === 0 ? props.film : row.value}</span>
              </Show>
            </div>
          </div>
        )}
      </For>
    </div>
  );
}

type Presentation = 'sheet' | 'edge';
const SPRING = '460ms cubic-bezier(0.22, 1, 0.36, 1)';

function Presented() {
  const [style, setStyle] = createSignal<Presentation>('sheet');
  const [open, setOpen] = createSignal(false);
  const [state, setState] = createSignal<State>(initial({ film: START }));
  const histogram = useHistogram();
  const editorWidth = RAIL + SCROLLER;
  const hidden = () => (style() === 'sheet' ? 'translateY(100%)' : `translateX(-${editorWidth + 30}px)`);
  return (
    <>
      <div class="mb-[10px] flex flex-wrap gap-[8px]">
        <For each={[['sheet', 'Bottom sheet'], ['edge', 'From the left edge']] as [Presentation, string][]}>
          {([value, label]) => (
            <button
              type="button"
              class="h-[44px] rounded-[12px] px-[14px] text-[16px] font-semibold"
              style={style() === value ? { background: '#fff', color: '#000' } : { background: '#2c2c2e', color: '#fff' }}
              onClick={() => {
                setOpen(false);
                setStyle(value);
              }}
            >
              {label}
            </button>
          )}
        </For>
        <button type="button" class="h-[44px] rounded-[12px] px-[18px] text-[16px] font-semibold text-white" style={{ background: ACCENT }} onClick={() => setOpen(!open())}>
          {open() ? 'Back' : 'Edit film'}
        </button>
      </div>
      <Fit>
        <div class="relative h-[480px] w-[720px] overflow-hidden bg-black font-sans select-none">
          <Picture film={state().film} custom={state().custom} crop={0} />
          <div
            class="absolute inset-0 origin-top"
            style={{
              transition: `transform ${SPRING}, filter ${SPRING}`,
              transform: open() && style() === 'sheet' ? 'scale(0.94)' : 'none',
              filter: open() ? 'brightness(0.55)' : 'none',
            }}
          >
            <RecipeColumn film="Classic Chrome" />
            <Show when={!open()}>
              <div class="absolute right-[10px] bottom-[10px]">
                <Hint glyph="Fn" label="EDIT FILM" tap={() => setOpen(true)} />
              </div>
            </Show>
          </div>
          <div
            class="absolute inset-0"
            style={{
              transition: `transform ${SPRING}, visibility 0s linear ${open() ? '0ms' : '460ms'}`,
              transform: open() ? 'none' : hidden(),
              visibility: open() ? 'visible' : 'hidden',
              top: style() === 'sheet' ? '12px' : '0',
            }}
          >
            <div
              class="absolute inset-0 overflow-hidden"
              style={style() === 'sheet' ? { 'border-radius': '16px 16px 0 0', width: `${editorWidth}px` } : { width: `${editorWidth}px` }}
            >
              <Editor s={state()} set={setState} histogram={histogram()} title={NEW_TITLE} />
            </div>
            <Show when={style() === 'sheet'}>
              <div class="absolute top-[5px] h-[5px] w-[40px] -translate-x-1/2 rounded-full bg-white/60" style={{ left: `${editorWidth / 2}px` }} />
            </Show>
          </div>
        </div>
      </Fit>
    </>
  );
}

/* ---- Review ------------------------------------------------------------------------- */

const STATES: { id: string; title: string; state: State }[] = [
  { id: 'curve', title: 'Inside the tone curve', state: initial({ focus: 0, knot: 7, inside: true }) },
  { id: 'color-mix', title: 'Color mix', state: initial({ focus: 5, scrollTo: 'mix' }) },
  { id: 'tone', title: 'Saturation by tone', state: initial({ focus: CONTROLS.findIndex((c) => c.kind === 'tone'), band: 4, inside: true, scrollTo: 'tone' }) },
  { id: 'copy', title: 'Copy one tool from a film', state: openPicker(initial({ header: true }), { kind: 'copy', tool: 'curve' }) },
  { id: 'compare', title: 'Compare with a film', state: initial({ compare: { b: 0, split: SPLIT } }) },
  { id: 'film-only', title: 'Custom setting off: the film alone', state: initial({ custom: false, area: 'custom', scrollTo: 'custom' }) },
  ...STRIP_STYLES.map(([strip, name]) => ({ id: `raw-${strip}`, title: `RAW files: ${name.toLowerCase()}`, state: initial({ crop: 3, area: 'strip', strip }) })),
];

function Static(props: { start: State }) {
  const [state, setState] = createSignal<State>(props.start);
  return (
    <Fit>
      <Screen state={state} set={setState} />
    </Fit>
  );
}

function Review() {
  return (
    <div class="min-h-dvh touch-manipulation bg-[#111] px-[12px] py-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="mb-[12px] text-[22px] font-semibold">Film editor</h1>
      <Live start={initial({ film: START })} />
      <h2 class="mt-[32px] mb-[10px] text-[17px] font-semibold">From the recipe editor</h2>
      <Presented />
      <For each={STATES}>
        {(entry) => (
          <section id={entry.id} class="mt-[28px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h2>
            <Static start={entry.state} />
          </section>
        )}
      </For>
    </div>
  );
}

function Fit(props: { children: JSX.Element }) {
  let host: HTMLDivElement | undefined;
  const [scale, setScale] = createSignal(1);
  onSettled(() => {
    const element = host;
    if (!element) return;
    const observer = new ResizeObserver(() => setScale(Math.min(1, element.clientWidth / 720)));
    observer.observe(element);
    return () => observer.disconnect();
  });
  return (
    <div ref={(element) => (host = element)} class="w-full max-w-[720px] overflow-hidden" style={{ height: `${480 * scale()}px` }}>
      <div class="origin-top-left" style={{ transform: `scale(${scale()})`, width: '720px' }}>
        {props.children}
      </div>
    </div>
  );
}

const host = document.getElementById('film-editor');
if (!host) throw new Error('Missing #film-editor');
if (import.meta.env.DEV) {
  host.replaceChildren();
  render(() => <Review />, host);
}
