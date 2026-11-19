import { render } from '@solidjs/web';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import './focal.css';

/* FILM EDITOR redesign mock: a film's controls as an editing sidebar over
   live view, in four layouts. The picture is developed the way the film
   stage develops it: a 3x3 matrix on linear RGB (the primaries), one shared
   tone curve of ten knots joined by cubic Bezier segments, the channels
   entering that curve at their own exposure, then a 2x2 on CbCr
   (saturation, hue, warm/cool) and a chroma gain chosen by output
   brightness in five bands. */

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

let sourcePromise: Promise<Source> | undefined;

function source(): Promise<Source> {
  sourcePromise ??= new Promise((resolve) => {
    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement('canvas');
      canvas.width = W;
      canvas.height = H;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('No 2D context');
      const scale = Math.max(W / image.width, H / image.height);
      const w = image.width * scale;
      const h = image.height * scale;
      context.drawImage(image, (W - w) / 2, (H - h) / 2, w, h);
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
  return sourcePromise;
}

const DECODE = Array.from({ length: 256 }, (_, v) => decode(v / 255));

function develop(f: Film, src: Source, out: ImageData) {
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
  const band = new Float32Array(256);
  const centers = [0, ...f.edges, 255];
  for (let y = 0; y < 256; y++) {
    let i = 0;
    while (i < 4 && y > f.edges[i]) i++;
    const lo = centers[i];
    const hi = centers[i + 1];
    const mid = (lo + hi) / 2;
    const next = y < mid ? Math.max(0, i - 1) : Math.min(4, i + 1);
    const nextMid = (centers[next] + centers[next + 1]) / 2;
    const t = next === i ? 0 : Math.min(0.5, Math.abs(y - mid) / Math.max(1, Math.abs(nextMid - mid)));
    band[y] = 1 + ((1 - t) * f.bands[i] + t * f.bands[next]) / 100;
  }
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
    o[p] = R2 * 255;
    o[p + 1] = G2 * 255;
    o[p + 2] = B2 * 255;
    o[p + 3] = 255;
  }
}

function Picture(props: { film: Film; onPick?: (x: number, y: number, u: number) => void }) {
  let canvas: HTMLCanvasElement | undefined;
  const [src, setSrc] = createSignal<Source>();
  let frame = 0;
  onSettled(() => {
    void source().then(setSrc);
  });
  createEffect(
    () => [props.film, src()] as const,
    ([film, s]) => {
      if (!s || !canvas) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const context = canvas?.getContext('2d');
        if (!context) return;
        const out = context.createImageData(W, H);
        develop(film, s, out);
        context.putImageData(out, 0, 0);
      });
    },
  );
  return (
    <canvas
      ref={(element) => (canvas = element)}
      width={W}
      height={H}
      class="absolute inset-0"
      onPointerDown={(e) => {
        const s = src();
        if (!props.onPick || !s) return;
        const box = e.currentTarget.getBoundingClientRect();
        const x = Math.round(((e.clientX - box.left) / box.width) * W);
        const y = Math.round(((e.clientY - box.top) / box.height) * H);
        const p = (y * W + x) * 4;
        const l = 0.2126 * DECODE[s.pixels[p]] + 0.7152 * DECODE[s.pixels[p + 1]] + 0.0722 * DECODE[s.pixels[p + 2]];
        props.onPick(x, y, axis(l * 4096));
      }}
    />
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
const PRIMARY_COLORS = ['#e5484d', '#46a758', '#3e63dd'];
const PRIMARY_NAMES = ['RED', 'GREEN', 'BLUE'];
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
      class="relative h-full touch-none overflow-hidden"
      style={{
        width: `${props.width}px`,
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
    >
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
    <div class="flex h-[57px] items-center gap-[8px] px-[8px]" style={{ width: `${props.width}px` }} onClick={() => props.onClick?.()}>
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

function BaseStrip(props: { film: Film; width: number; focused: boolean; set: (base: number) => void }) {
  return (
    <div class="relative flex h-full items-center justify-center" style={{ width: `${props.width}px` }}>
      <span
        class={`flex h-[37px] items-center rounded-[10px] px-[10px] font-medium whitespace-nowrap ${SMALL}`}
        style={{ background: '#fff', color: '#000' }}
      >
        {BASES[props.film.base].name}
      </span>
      <Show when={props.film.base > 0}>
        <button type="button" class="absolute left-0 w-[16px] text-[24px] text-white" onClick={() => props.set(props.film.base - 1)}>
          ‹
        </button>
      </Show>
      <Show when={props.film.base < BASES.length - 1}>
        <button type="button" class="absolute right-0 w-[16px] text-[24px] text-white" onClick={() => props.set(props.film.base + 1)}>
          ›
        </button>
      </Show>
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

/** A small picture of the curve for a row's track. */
function CurveSpark(props: { film: Film; opened: Film; width: number; height: number }) {
  return (
    <svg width={props.width} height={props.height} class="block">
      <path d={curvePath(props.opened, props.width, props.height)} fill="none" stroke="rgb(255 255 255 / 0.3)" stroke-width="1.5" />
      <path d={curvePath(props.film, props.width, props.height)} fill="none" stroke="#fff" stroke-width="2.5" />
    </svg>
  );
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

/** Saturation by brightness: five bands between four edges, drawn over the
 * picture's brightness. A finger drags a band's bar or an edge. */
function BandGraph(props: { film: Film; opened: Film; width: number; height: number; focus: number; set: (film: Film) => void; focusBand: (band: number) => void }) {
  const pad = 14;
  const w = () => props.width - 2 * pad;
  const h = () => props.height - 2 * pad;
  const X = (y: number) => pad + (y / 255) * w();
  const mid = () => pad + h() / 2;
  const Yb = (pct: number) => mid() - (pct / 100) * (h() / 2);
  const bounds = () => [0, ...props.film.edges, 255];
  let drag: { band: number; y: number; start: number } | undefined;
  return (
    <svg
      width={props.width}
      height={props.height}
      class="block touch-none"
      onPointerDown={(e) => {
        const element = e.currentTarget;
        element.setPointerCapture(e.pointerId);
        const box = element.getBoundingClientRect();
        const x = ((e.clientX - box.left) / box.width) * props.width;
        const y = ((e.clientY - box.top) / box.height) * props.height;
        const b = bounds();
        let band = 0;
        while (band < 4 && x > X(b[band + 1])) band++;
        props.focusBand(band);
        drag = { band, y, start: props.film.bands[band] };
      }}
      onPointerMove={(e) => {
        if (!drag) return;
        const box = e.currentTarget.getBoundingClientRect();
        const y = ((e.clientY - box.top) / box.height) * props.height;
        const bands = [...props.film.bands];
        bands[drag.band] = Math.max(-100, Math.min(100, Math.round((drag.start + ((drag.y - y) / (h() / 2)) * 100) / 5) * 5));
        props.set({ ...props.film, bands });
      }}
      onPointerUp={() => (drag = undefined)}
    >
      <defs>
        <linearGradient id="band-ramp" x1="0" x2="1">
          <stop offset="0" stop-color="#000" />
          <stop offset="1" stop-color="#fff" />
        </linearGradient>
      </defs>
      <rect x={pad} y={pad + h() - 8} width={w()} height="8" fill="url(#band-ramp)" opacity="0.8" />
      <line x1={pad} x2={pad + w()} y1={mid()} y2={mid()} stroke="rgb(255 255 255 / 0.35)" stroke-dasharray="4 4" />
      <For each={props.film.bands}>
        {(pct, i) => {
          const b = () => bounds();
          const focused = () => i() === props.focus;
          const changed = () => pct !== props.opened.bands[i()];
          return (
            <>
              <rect
                x={X(b()[i()]) + 3}
                y={Math.min(mid(), Yb(pct))}
                width={Math.max(2, X(b()[i() + 1]) - X(b()[i()]) - 6)}
                height={Math.max(2, Math.abs(Yb(pct) - mid()))}
                rx="4"
                fill={changed() ? ACCENT : focused() ? '#fff' : 'rgb(255 255 255 / 0.5)'}
              />
              <Show when={focused()}>
                <rect x={X(b()[i()]) + 1} y={pad} width={X(b()[i() + 1]) - X(b()[i()]) - 2} height={h() - 10} rx="6" fill="none" stroke="#fff" stroke-width="2" />
              </Show>
            </>
          );
        }}
      </For>
      <For each={props.film.edges}>
        {(edge) => <line x1={X(edge)} x2={X(edge)} y1={pad} y2={pad + h()} stroke="rgb(255 255 255 / 0.4)" stroke-width="1.5" />}
      </For>
    </svg>
  );
}

function Legend(props: { hints: [string, string][]; left?: boolean }) {
  return (
    <div class="absolute bottom-[10px] flex gap-[4px]" style={props.left ? { left: '10px' } : { right: '10px' }}>
      <For each={props.hints}>
        {([key, label]) => (
          <div class="flex h-[34px] items-center gap-[8px] rounded-[12px] px-[7px] whitespace-nowrap" style={GLASS}>
            <span class="flex h-[24px] min-w-[24px] items-center justify-center rounded-[6px] bg-white px-[4px] text-[18px] leading-none font-semibold text-black">{key}</span>
            <Show when={label}>
              <span class={`pr-[3px] font-light tracking-[0.04em] ${SMALL}`}>{label}</span>
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}

function Swatches(props: { film: Film; focus: number; size: number; pick: (primary: number) => void }) {
  return (
    <div class="flex items-center justify-center gap-[14px]">
      <For each={PRIMARY_COLORS}>
        {(color, i) => {
          const p = () => props.film.primaries[i()];
          const changed = () => p().hue !== 0 || p().sat !== 0;
          return (
            <button
              type="button"
              class="relative rounded-full"
              style={{
                width: `${props.size}px`,
                height: `${props.size}px`,
                background: `color-mix(in oklch, ${color}, ${p().hue > 0 ? '#ffd60a' : '#bf5af2'} ${Math.abs(p().hue) * 2}%)`,
                filter: `saturate(${1 + p().sat / 100})`,
                'box-shadow': i() === props.focus ? '0 0 0 3px #1c1c1e, 0 0 0 6px #fff' : undefined,
              }}
              onClick={() => props.pick(i())}
            >
              <Show when={changed()}>
                <span class="absolute -right-[3px] -bottom-[3px] size-[14px] rounded-full" style={{ background: ACCENT, 'box-shadow': '0 0 0 2px #1c1c1e' }} />
              </Show>
            </button>
          );
        }}
      </For>
    </div>
  );
}

/* ---- Tools ----------------------------------------------------------------------- */

type Tool = 'base' | 'curve' | 'color' | 'primaries' | 'bands' | 'channels';
const TOOL_NAMES: Record<Tool, string> = {
  base: 'BASE FILM',
  curve: 'TONE CURVE',
  color: 'COLOR',
  primaries: 'PRIMARIES',
  bands: 'SAT BY TONE',
  channels: 'CHANNELS',
};
const TOOLS: Tool[] = ['base', 'curve', 'color', 'primaries', 'bands', 'channels'];

interface State {
  film: Film;
  opened: Film;
  tool: Tool;
  /** The focused control within the tool. */
  control: number;
  knot: number;
  bend: boolean;
  primary: number;
  band: number;
  /** Layout A: a tool open in the fly-out. Layout B: the open section. */
  open: Tool | null;
  picked?: { x: number; y: number; u: number };
}

const START = baseFilm(3);

function initial(over: Partial<State>): State {
  return { film: START, opened: START, tool: 'curve', control: 0, knot: 4, bend: false, primary: 0, band: 1, open: null, ...over };
}

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

type Control = { name: string; get: (f: Film) => number; put: (f: Film, v: number) => Film; min: number; max: number; step: number };

const COLOR_CONTROLS: Control[] = [
  { name: 'SATURATION', get: (f) => f.saturation, put: (f, v) => ({ ...f, saturation: v }), min: -50, max: 50, step: 5 },
  { name: 'HUE', get: (f) => f.hue, put: (f, v) => ({ ...f, hue: v }), min: -15, max: 15, step: 1 },
  { name: 'WARM/COOL', get: (f) => f.warmth, put: (f, v) => ({ ...f, warmth: v }), min: -50, max: 50, step: 5 },
];
const CHANNEL_CONTROLS: Control[] = [
  { name: 'RED CURVE', get: (f) => f.red, put: (f, v) => ({ ...f, red: v }), min: -5, max: 5, step: 1 },
  { name: 'BLUE CURVE', get: (f) => f.blue, put: (f, v) => ({ ...f, blue: v }), min: -5, max: 5, step: 1 },
];
const primaryControls = (p: number): Control[] => [
  {
    name: 'HUE',
    get: (f) => f.primaries[p].hue,
    put: (f, v) => ({ ...f, primaries: f.primaries.map((q, i) => (i === p ? { ...q, hue: v } : q)) }),
    min: -30,
    max: 30,
    step: 2,
  },
  {
    name: 'SATURATION',
    get: (f) => f.primaries[p].sat,
    put: (f, v) => ({ ...f, primaries: f.primaries.map((q, i) => (i === p ? { ...q, sat: v } : q)) }),
    min: -50,
    max: 50,
    step: 5,
  },
];

/** Keys for whichever tool has focus: UP/DOWN move between its controls,
 * LEFT/RIGHT change the focused one. On the curve LEFT/RIGHT pick a knot and
 * UP/DOWN move it (the dial too); OK toggles BEND. */
function toolKey(s: State, tool: Tool, key: string): State | undefined {
  const f = s.film;
  const nudge = (control: Control, d: number) => ({
    ...s,
    film: control.put(f, Math.max(control.min, Math.min(control.max, control.get(f) + d * control.step))),
  });
  const list = (controls: Control[]) => {
    if (key === 'ArrowLeft' || key === 'ArrowRight') return nudge(controls[s.control] ?? controls[0], key === 'ArrowRight' ? 1 : -1);
    if (key === 'ArrowUp' && s.control > 0) return { ...s, control: s.control - 1 };
    if (key === 'ArrowDown' && s.control < controls.length - 1) return { ...s, control: s.control + 1 };
    return undefined;
  };
  switch (tool) {
    case 'base':
      if (key === 'ArrowLeft' || key === 'ArrowRight') {
        const base = Math.max(0, Math.min(BASES.length - 1, f.base + (key === 'ArrowRight' ? 1 : -1)));
        return { ...s, film: baseFilm(base), opened: baseFilm(base) };
      }
      return undefined;
    case 'curve':
      if (key === 'ArrowLeft' || key === 'ArrowRight') return { ...s, knot: Math.max(0, Math.min(9, s.knot + (key === 'ArrowRight' ? 1 : -1))) };
      if (key === 'ArrowUp' || key === 'ArrowDown') {
        const d = key === 'ArrowUp' ? 1 : -1;
        if (s.bend) {
          const tilt = [...f.tilt];
          tilt[s.knot] = Math.round((tilt[s.knot] + d * 0.1) * 10) / 10;
          return { ...s, film: { ...f, tilt } };
        }
        const knots = [...f.knots];
        knots[s.knot] = fromDisp(disp(knots[s.knot]) + d / 70);
        return { ...s, film: { ...f, knots } };
      }
      if (key === 'Enter') return { ...s, bend: !s.bend };
      return undefined;
    case 'color':
      return list(COLOR_CONTROLS);
    case 'channels':
      return list(CHANNEL_CONTROLS);
    case 'primaries':
      if (s.control === 0 && (key === 'ArrowLeft' || key === 'ArrowRight'))
        return { ...s, primary: Math.max(0, Math.min(2, s.primary + (key === 'ArrowRight' ? 1 : -1))) };
      if (s.control > 0 && (key === 'ArrowLeft' || key === 'ArrowRight')) {
        const control = primaryControls(s.primary)[s.control - 1];
        return nudge(control, key === 'ArrowRight' ? 1 : -1);
      }
      if (key === 'ArrowUp' && s.control > 0) return { ...s, control: s.control - 1 };
      if (key === 'ArrowDown' && s.control < 2) return { ...s, control: s.control + 1 };
      return undefined;
    case 'bands':
      if (key === 'ArrowLeft' || key === 'ArrowRight') return { ...s, band: Math.max(0, Math.min(4, s.band + (key === 'ArrowRight' ? 1 : -1))) };
      if (key === 'ArrowUp' || key === 'ArrowDown') {
        const bands = [...f.bands];
        bands[s.band] = Math.max(-100, Math.min(100, bands[s.band] + (key === 'ArrowUp' ? 5 : -5)));
        return { ...s, film: { ...f, bands } };
      }
      return undefined;
  }
}

function Controls(props: { s: State; controls: Control[]; width: number; offset?: number; set: (s: State) => void }) {
  return (
    <For each={props.controls}>
      {(control, i) => (
        <Row name={control.name} focused={props.s.control === i() + (props.offset ?? 0)} width={props.width} onClick={() => props.set({ ...props.s, control: i() + (props.offset ?? 0) })}>
          <Ruler
            value={control.get(props.s.film)}
            min={control.min}
            max={control.max}
            step={control.step}
            width={props.width - 170}
            focused={props.s.control === i() + (props.offset ?? 0)}
            changed={control.get(props.s.film) !== control.get(props.s.opened)}
            set={(v) => props.set({ ...props.s, control: i() + (props.offset ?? 0), film: control.put(props.s.film, v) })}
          />
        </Row>
      )}
    </For>
  );
}

/** A tool's full panel, as the fly-out (A), the open section (B) and the rail's panel (C) show it. */
function ToolPanel(props: { s: State; tool: Tool; width: number; graphHeight: number; histogram: number[]; set: (s: State) => void }) {
  const s = () => props.s;
  return (
    <div class="flex flex-col gap-[6px]" style={{ width: `${props.width}px` }}>
      <Show when={props.tool === 'base'}>
        <Row name="COPY FROM" focused width={props.width}>
          <BaseStrip film={s().film} width={props.width - 170} focused set={(base) => props.set({ ...s(), film: baseFilm(base), opened: baseFilm(base) })} />
        </Row>
      </Show>
      <Show when={props.tool === 'curve'}>
        <CurveGraph
          film={s().film}
          opened={s().opened}
          histogram={props.histogram}
          width={props.width}
          height={props.graphHeight}
          focus={s().knot}
          bend={s().bend}
          picked={s().picked?.u}
          set={(film) => props.set({ ...s(), film })}
          focusKnot={(knot) => props.set({ ...s(), knot })}
        />
        <div class={`px-[10px] font-medium tabular-nums ${SMALL}`}>{knotReadout(s().film, s().knot, s().bend)}</div>
      </Show>
      <Show when={props.tool === 'color'}>
        <Controls s={s()} controls={COLOR_CONTROLS} width={props.width} set={props.set} />
      </Show>
      <Show when={props.tool === 'channels'}>
        <Controls s={s()} controls={CHANNEL_CONTROLS} width={props.width} set={props.set} />
      </Show>
      <Show when={props.tool === 'primaries'}>
        <div class="flex h-[57px] items-center justify-center rounded-[13px]" style={TRACK(s().control === 0)}>
          <Swatches film={s().film} focus={s().primary} size={38} pick={(primary) => props.set({ ...s(), primary, control: 0 })} />
        </div>
        <div class={`px-[10px] font-medium ${SMALL}`}>{PRIMARY_NAMES[s().primary]}</div>
        <Controls s={s()} controls={primaryControls(s().primary)} width={props.width} offset={1} set={props.set} />
      </Show>
      <Show when={props.tool === 'bands'}>
        <BandGraph
          film={s().film}
          opened={s().opened}
          width={props.width}
          height={Math.min(props.graphHeight, 200)}
          focus={s().band}
          set={(film) => props.set({ ...s(), film })}
          focusBand={(band) => props.set({ ...s(), band })}
        />
        <div class={`px-[10px] font-medium tabular-nums ${SMALL}`}>
          {BAND_NAMES[s().band]} · {signed(s().film.bands[s().band])}%
        </div>
      </Show>
    </div>
  );
}

function hintsFor(tool: Tool, bend: boolean): [string, string][] {
  switch (tool) {
    case 'curve':
      return bend
        ? [['◀▶', 'POINT'], ['▲▼', 'BEND'], ['OK', 'MOVE'], ['BACK', '']]
        : [['◀▶', 'POINT'], ['▲▼', 'MOVE'], ['OK', 'BEND'], ['BACK', '']];
    case 'bands':
      return [['◀▶', 'BAND'], ['▲▼', 'SATURATION'], ['BACK', '']];
    case 'base':
      return [['◀▶', 'FILM'], ['BACK', '']];
    default:
      return [['▲▼', 'SELECT'], ['◀▶', 'ADJUST'], ['BACK', '']];
  }
}

/* ---- Layout A: rows and fly-outs -------------------------------------------------- */

/* The recipe editor's column, one 57-pixel row per control. Rows whose
   control needs room (the curve, the primaries, saturation by tone) show a
   summary in their track and open a fly-out beside the column, as WB SHIFT
   does. */
const ROWS_A: { tool: Tool; control?: Control }[] = [
  { tool: 'base' },
  { tool: 'curve' },
  { tool: 'color', control: COLOR_CONTROLS[0] },
  { tool: 'color', control: COLOR_CONTROLS[1] },
  { tool: 'color', control: COLOR_CONTROLS[2] },
  { tool: 'primaries' },
  { tool: 'bands' },
  { tool: 'channels', control: CHANNEL_CONTROLS[0] },
  { tool: 'channels', control: CHANNEL_CONTROLS[1] },
];
const COLUMN_A = 360;

function LayoutA(props: { s: State; set: (s: State) => void; histogram: number[]; title?: { name: string; from?: string }; opaque?: boolean }) {
  const s = () => props.s;
  const head = () => (props.title ? 56 : 0);
  const scroll = () => Math.max(0, Math.min(ROWS_A.length * 57 + head() - 480 + 8, (s().control - 3) * 57));
  return (
    <>
      <div
        class="absolute inset-y-0 left-0 overflow-hidden"
        style={{ ...COLUMN, ...(props.opaque ? { background: 'rgb(26 26 28 / 0.96)' } : {}), width: `${COLUMN_A}px` }}
      >
        <div class="transition-transform duration-150" style={{ transform: `translateY(${4 - scroll()}px)` }}>
          <Show when={props.title}>
            {(title) => (
              <div class="flex h-[56px] items-baseline gap-[10px] px-[12px] pt-[14px]" style={{ 'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.16)' }}>
                <span class={`font-semibold ${TEXT}`}>{title().name}</span>
                <Show when={title().from}>
                  <span class={`text-white/60 ${SMALL}`}>{title().from}</span>
                </Show>
              </div>
            )}
          </Show>
          <For each={ROWS_A}>
            {(row, i) => {
              const focused = () => (s().open ? !row.control && row.tool === s().open : s().control === i());
              const name = () => row.control?.name ?? TOOL_NAMES[row.tool];
              return (
                <Row name={name()} focused={focused()} width={COLUMN_A} onClick={() => props.set({ ...s(), control: i(), open: row.control || row.tool === 'base' ? null : row.tool })}>
                  <Show when={row.control}>
                    {(control) => (
                      <Ruler
                        value={control().get(s().film)}
                        min={control().min}
                        max={control().max}
                        step={control().step}
                        width={COLUMN_A - 170}
                        focused={focused()}
                        changed={control().get(s().film) !== control().get(s().opened)}
                        set={(v) => props.set({ ...s(), control: i(), film: control().put(s().film, v) })}
                      />
                    )}
                  </Show>
                  <Show when={row.tool === 'base'}>
                    <BaseStrip film={s().film} width={COLUMN_A - 170} focused={focused()} set={(base) => props.set({ ...s(), film: baseFilm(base), opened: baseFilm(base) })} />
                  </Show>
                  <Show when={row.tool === 'curve'}>
                    <div class="flex h-full items-center justify-between px-[10px]">
                      <CurveSpark film={s().film} opened={s().opened} width={120} height={36} />
                      <span class="text-[24px]">›</span>
                    </div>
                  </Show>
                  <Show when={row.tool === 'primaries'}>
                    <div class="flex h-full items-center justify-between px-[10px]">
                      <Swatches film={s().film} focus={-1} size={24} pick={() => undefined} />
                      <span class="text-[24px]">›</span>
                    </div>
                  </Show>
                  <Show when={row.tool === 'bands'}>
                    <div class="flex h-full items-center justify-between px-[10px]">
                      <svg width="120" height="36">
                        <For each={s().film.bands}>
                          {(b, k) => <rect x={k() * 24 + 2} y={b >= 0 ? 18 - (b / 100) * 16 : 18} width="20" height={Math.max(2, (Math.abs(b) / 100) * 16)} rx="2" fill={b === s().opened.bands[k()] ? '#fff' : ACCENT} />}
                        </For>
                      </svg>
                      <span class="text-[24px]">›</span>
                    </div>
                  </Show>
                </Row>
              );
            }}
          </For>
        </div>
      </div>
      <Show when={s().open}>
        {(tool) => (
          <div class="absolute top-[8px] left-[368px] flex flex-col gap-[8px] rounded-[16px] p-[10px]" style={GLASS}>
            <div class={`px-[6px] pt-[2px] font-semibold ${TEXT}`}>{TOOL_NAMES[tool()]}</div>
            <ToolPanel s={s()} tool={tool()} width={324} graphHeight={300} histogram={props.histogram} set={props.set} />
          </div>
        )}
      </Show>
      <Legend hints={s().open ? hintsFor(s().open ?? 'curve', s().bend) : [['◀▶', 'ADJUST'], ['OK', 'OPEN']]} />
    </>
  );
}

function keyA(s: State, key: string): State | undefined {
  if (s.open) {
    if (key === 'Escape' || key === 'Backspace') return { ...s, open: null, bend: false, control: ROWS_A.findIndex((r) => r.tool === s.open) };
    return toolKey(s, s.open, key);
  }
  const row = ROWS_A[s.control];
  if (key === 'ArrowUp' && s.control > 0) return { ...s, control: s.control - 1 };
  if (key === 'ArrowDown' && s.control < ROWS_A.length - 1) return { ...s, control: s.control + 1 };
  if (key === 'Enter' && !row.control && row.tool !== 'base') return { ...s, open: row.tool, control: 0 };
  if ((key === 'ArrowLeft' || key === 'ArrowRight') && row.control) {
    const c = row.control;
    const v = Math.max(c.min, Math.min(c.max, c.get(s.film) + (key === 'ArrowRight' ? c.step : -c.step)));
    return { ...s, film: c.put(s.film, v) };
  }
  if (row.tool === 'base') return toolKey(s, 'base', key);
  return undefined;
}

/* ---- Layout B: sections of any height --------------------------------------------- */

/* Darkroom's and Capture One's sidebar: a section per tool, its header a
   row with the tool's name; the open section shows its whole panel inline,
   at whatever height it needs, and the column scrolls. OK opens or closes
   the focused section; inside it, the keys work its controls. */
const COLUMN_B = 360;

function sectionHeight(s: State, tool: Tool): number {
  if (s.open !== tool) return 50;
  switch (tool) {
    case 'curve':
      return 50 + 236 + 36;
    case 'bands':
      return 50 + 200 + 40;
    case 'primaries':
      return 50 + 57 + 30 + 2 * 63;
    case 'color':
      return 50 + 3 * 63;
    case 'channels':
      return 50 + 2 * 63;
    case 'base':
      return 50 + 63;
  }
}

function LayoutB(props: { s: State; set: (s: State) => void; histogram: number[] }) {
  const s = () => props.s;
  const top = (tool: Tool) => TOOLS.slice(0, TOOLS.indexOf(tool)).reduce((sum, t) => sum + sectionHeight(s(), t), 0);
  const total = () => TOOLS.reduce((sum, t) => sum + sectionHeight(s(), t), 0);
  const scroll = () => Math.max(0, Math.min(total() - 474, top(s().tool) - (s().open ? 0 : 120)));
  return (
    <>
      <div class="absolute inset-y-0 left-0 overflow-hidden" style={{ ...COLUMN, width: `${COLUMN_B}px` }}>
        <div class="px-[8px] pt-[6px] transition-transform duration-150" style={{ transform: `translateY(${-scroll()}px)` }}>
          <For each={TOOLS}>
            {(tool) => {
              const open = () => s().open === tool;
              const focused = () => s().tool === tool;
              const edited = () => JSON.stringify(sectionValue(s().film, tool)) !== JSON.stringify(sectionValue(s().opened, tool));
              return (
                <div style={{ height: `${sectionHeight(s(), tool)}px`, 'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.12)' }}>
                  <div
                    class="flex h-[50px] items-center gap-[10px] rounded-[12px] px-[10px]"
                    style={focused() && !open() ? TRACK(true) : {}}
                    onClick={() => props.set({ ...s(), tool, open: open() ? null : tool, control: tool === 'primaries' ? 0 : 0 })}
                  >
                    <span class="w-[20px] text-[21px] text-white/70">{open() ? '▾' : '▸'}</span>
                    <span class={`grow font-medium tracking-[0.04em] ${TEXT}`}>{TOOL_NAMES[tool]}</span>
                    <Show when={tool === 'base'}>
                      <span class={`text-white/70 ${SMALL}`}>{BASES[s().film.base].name}</span>
                    </Show>
                    <Show when={edited()}>
                      <span class="size-[10px] rounded-full" style={{ background: ACCENT }} />
                    </Show>
                  </div>
                  <Show when={open()}>
                    <ToolPanel s={s()} tool={tool} width={COLUMN_B - 16} graphHeight={236} histogram={props.histogram} set={props.set} />
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </div>
      <Legend hints={s().open ? hintsFor(s().open ?? 'curve', s().bend) : [['▲▼', 'SELECT'], ['OK', 'OPEN']]} />
    </>
  );
}

function sectionValue(f: Film, tool: Tool): unknown {
  switch (tool) {
    case 'base':
      return f.base;
    case 'curve':
      return [f.knots, f.tilt];
    case 'color':
      return [f.saturation, f.hue, f.warmth];
    case 'primaries':
      return f.primaries;
    case 'bands':
      return [f.bands, f.edges];
    case 'channels':
      return [f.red, f.blue];
  }
}

function keyB(s: State, key: string): State | undefined {
  if (s.open) {
    if (key === 'Escape' || key === 'Backspace') return { ...s, open: null, bend: false };
    return toolKey(s, s.open, key);
  }
  const i = TOOLS.indexOf(s.tool);
  if (key === 'ArrowUp' && i > 0) return { ...s, tool: TOOLS[i - 1] };
  if (key === 'ArrowDown' && i < TOOLS.length - 1) return { ...s, tool: TOOLS[i + 1] };
  if (key === 'Enter') return { ...s, open: s.tool, control: 0 };
  return undefined;
}

/* ---- Layout C: tool rail ---------------------------------------------------------- */

/* Darkroom and Lightroom on a phone: a rail of tools down the screen's
   edge; the chosen tool fills the panel beside it at full height. UP/DOWN
   on the rail choose a tool, RIGHT enters its panel, BACK returns. */
const RAIL = 68;
const PANEL_C = 360;

function ToolIcon(props: { tool: Tool; color: string }) {
  const stroke = () => ({ fill: 'none', stroke: props.color, 'stroke-width': 2.5, 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const });
  return (
    <svg width="34" height="34" viewBox="0 0 34 34">
      <Show when={props.tool === 'base'}>
        <rect x="6" y="5" width="22" height="24" rx="2" {...stroke()} />
        <path d="M6 11h22M6 23h22" {...stroke()} />
      </Show>
      <Show when={props.tool === 'curve'}>
        <rect x="5" y="5" width="24" height="24" rx="3" {...stroke()} stroke-opacity="0.5" />
        <path d="M7 27C14 26 13 9 27 7" {...stroke()} />
      </Show>
      <Show when={props.tool === 'color'}>
        <circle cx="17" cy="17" r="11" {...stroke()} />
        <path d="M17 6a11 11 0 0 1 0 22z" fill={props.color} />
      </Show>
      <Show when={props.tool === 'primaries'}>
        <circle cx="12" cy="13" r="6.5" {...stroke()} />
        <circle cx="22" cy="13" r="6.5" {...stroke()} />
        <circle cx="17" cy="22" r="6.5" {...stroke()} />
      </Show>
      <Show when={props.tool === 'bands'}>
        <path d="M6 24v-4M12 24v-9M18 24v-13M24 24v-7M29 24v-3" {...stroke()} />
        <path d="M5 28h24" {...stroke()} stroke-opacity="0.5" />
      </Show>
      <Show when={props.tool === 'channels'}>
        <path d="M6 27C13 25 15 12 28 8" {...stroke()} stroke={PRIMARY_COLORS[0]} />
        <path d="M6 27C15 26 18 13 28 10" {...stroke()} stroke={PRIMARY_COLORS[2]} />
      </Show>
    </svg>
  );
}

function LayoutC(props: { s: State; set: (s: State) => void; histogram: number[] }) {
  const s = () => props.s;
  const inPanel = () => s().open !== null;
  return (
    <>
      <div class="absolute inset-y-0 left-0 flex flex-col items-center gap-[6px] pt-[10px]" style={{ ...COLUMN, width: `${RAIL}px` }}>
        <For each={TOOLS}>
          {(tool) => {
            const chosen = () => s().tool === tool;
            const edited = () => JSON.stringify(sectionValue(s().film, tool)) !== JSON.stringify(sectionValue(s().opened, tool));
            return (
              <button
                type="button"
                class="relative flex size-[56px] items-center justify-center rounded-[14px]"
                style={chosen() ? { background: inPanel() ? 'rgb(255 255 255 / 0.17)' : '#fff' } : {}}
                onClick={() => props.set({ ...s(), tool, open: null, control: 0, bend: false })}
              >
                <ToolIcon tool={tool} color={chosen() && !inPanel() ? '#000' : '#fff'} />
                <Show when={edited()}>
                  <span class="absolute top-[6px] right-[6px] size-[9px] rounded-full" style={{ background: ACCENT }} />
                </Show>
              </button>
            );
          }}
        </For>
      </div>
      <div class="absolute inset-y-0 flex flex-col gap-[8px] px-[8px] pt-[10px]" style={{ ...COLUMN, left: `${RAIL}px`, width: `${PANEL_C}px` }} onClick={() => !inPanel() && props.set({ ...s(), open: s().tool })}>
        <div class={`px-[6px] pt-[4px] pb-[4px] font-semibold ${TEXT}`}>{TOOL_NAMES[s().tool]}</div>
        <div style={{ opacity: inPanel() ? 1 : 0.85 }}>
          <ToolPanel s={s()} tool={s().tool} width={PANEL_C - 16} graphHeight={300} histogram={props.histogram} set={(next) => props.set({ ...next, open: next.tool })} />
        </div>
      </div>
      <Legend hints={inPanel() ? hintsFor(s().tool, s().bend).slice(0, 2) : [['▲▼', 'TOOL'], ['▶', 'EDIT']]} />
    </>
  );
}

function keyC(s: State, key: string): State | undefined {
  if (s.open) {
    if (key === 'Escape' || key === 'Backspace') return { ...s, open: null, bend: false };
    if (key === 'ArrowLeft' && (s.tool === 'color' || s.tool === 'channels' || s.tool === 'primaries') && s.control === 0 && s.tool !== 'primaries')
      return { ...s, open: null };
    return toolKey(s, s.open, key);
  }
  const i = TOOLS.indexOf(s.tool);
  if (key === 'ArrowUp' && i > 0) return { ...s, tool: TOOLS[i - 1] };
  if (key === 'ArrowDown' && i < TOOLS.length - 1) return { ...s, tool: TOOLS[i + 1] };
  if (key === 'ArrowRight' || key === 'Enter') return { ...s, open: s.tool, control: 0 };
  return undefined;
}

/* ---- Layout D: on the picture ----------------------------------------------------- */

/* Apple Photos' and Lightroom's targeted adjustment, on the live picture:
   the curve is drawn over the whole screen. Touch a tone in the picture
   and drag up or down: the knot for that brightness moves, and the curve
   marks where the touched tone falls. Knots can also be dragged directly,
   and the keys work as in the curve panel. */
function LayoutD(props: { s: State; set: (s: State) => void; histogram: number[] }) {
  const s = () => props.s;
  let drag: { y: number; start: number; knot: number } | undefined;
  const nearest = (u: number) => {
    let k = 0;
    for (let i = 1; i < 10; i++) if (Math.abs(US[i] - u) < Math.abs(US[k] - u)) k = i;
    return k;
  };
  return (
    <>
      <div
        class="absolute inset-0 touch-none"
        onPointerDown={(e) => {
          const element = e.currentTarget;
          const box = element.getBoundingClientRect();
          const x = ((e.clientX - box.left) / box.width) * W;
          const y = ((e.clientY - box.top) / box.height) * H;
          void source().then((src) => {
            const p = (Math.round(y) * W + Math.round(x)) * 4;
            const l = 0.2126 * DECODE[src.pixels[p]] + 0.7152 * DECODE[src.pixels[p + 1]] + 0.0722 * DECODE[src.pixels[p + 2]];
            const u = axis(l * 4096);
            const knot = nearest(u);
            drag = { y, start: s().film.knots[knot], knot };
            props.set({ ...s(), knot, picked: { x, y, u } });
          });
          element.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!drag) return;
          const box = e.currentTarget.getBoundingClientRect();
          const y = ((e.clientY - box.top) / box.height) * H;
          const knots = [...s().film.knots];
          knots[drag.knot] = fromDisp(disp(drag.start) + (drag.y - y) / 400);
          props.set({ ...s(), film: { ...s().film, knots } });
        }}
        onPointerUp={() => (drag = undefined)}
      />
      <div class="pointer-events-none absolute top-[14px] left-[14px] opacity-90" style={{ filter: 'drop-shadow(0 1px 2px rgb(0 0 0 / 0.8))' }}>
        <CurveGraph
          film={s().film}
          opened={s().opened}
          histogram={props.histogram}
          width={692}
          height={400}
          focus={s().knot}
          bend={s().bend}
          picked={s().picked?.u}
          set={() => undefined}
          focusKnot={() => undefined}
          bare
        />
      </div>
      <Show when={s().picked}>
        {(p) => (
          <div class="pointer-events-none absolute size-[44px] -translate-x-1/2 -translate-y-1/2 rounded-full" style={{ left: `${p().x}px`, top: `${p().y}px`, 'box-shadow': '0 0 0 3px #ffd60a, 0 0 0 5px rgb(0 0 0 / 0.5)' }} />
        )}
      </Show>
      <div class={`absolute top-[12px] right-[12px] rounded-[12px] px-[12px] py-[8px] font-medium tabular-nums ${SMALL}`} style={GLASS}>
        {knotReadout(s().film, s().knot, s().bend)}
      </div>
      <Legend left hints={[['◀▶', 'POINT'], ['▲▼', 'MOVE'], ['OK', 'BEND'], ['Fn', 'COLOR']]} />
    </>
  );
}

/* ---- From the recipe editor ------------------------------------------------------- */

/* The recipe editor's FILM SIMULATION row opens the film editor with the
   key WB SHIFT uses. A built-in film opens as a new film copied from it; a
   card film opens as itself. The film editor is a column too, so it
   presents as a sheet from the left edge: the recipe column recedes and
   dims, and peeks out from behind its right edge. BACK at the film
   editor's top level returns. */

const STOCK = '/review/stock-icons/x70-110';
const stockIcon = (id: number) => `${STOCK}/ff${id.toString(16).padStart(6, '0')}.svg`;
const RECIPE_WIDTH = 280;
const PEEK = 48;

type RecipeRow = { name: string; icon: number; value?: string; level?: number };
const RECIPE_ROWS: RecipeRow[] = [
  { name: 'FILM SIMULATION', icon: 0x5f3 },
  { name: 'WHITE BALANCE', icon: 0x609, value: 'Auto' },
  { name: 'DYNAMIC RANGE', icon: 0x5f1, value: 'DR100' },
  { name: 'HIGHLIGHT TONE', icon: 0x5f6, level: 1 },
  { name: 'SHADOW TONE', icon: 0x607, level: -1 },
  { name: 'COLOR', icon: 0x5f0, level: 0 },
  { name: 'SHARPNESS', icon: 0x605, level: 0 },
  { name: 'NOISE REDUCTION', icon: 0x603, level: -2 },
];

function StockIcon(props: { id: number; color: string }) {
  return (
    <span
      class="block size-[38px]"
      style={{
        background: props.color,
        'mask-image': `url(${stockIcon(props.id)})`,
        'mask-size': 'contain',
        'mask-repeat': 'no-repeat',
        'mask-position': 'center',
      }}
    />
  );
}

function RecipeColumn(props: { focus: number; film: string; changed: boolean }) {
  return (
    <div class="absolute inset-y-0 left-0 overflow-hidden pt-[4px]" style={{ ...COLUMN, width: `${RECIPE_WIDTH}px` }}>
      <For each={RECIPE_ROWS}>
        {(row, i) => {
          const focused = () => props.focus === i();
          return (
            <div class="flex h-[57px] items-center gap-[4px] pr-[6px] pl-[10px]">
              <StockIcon id={row.icon} color={focused() ? '#fff' : 'rgb(255 255 255 / 0.72)'} />
              <div class="ml-[8px] flex h-[47px] grow items-center justify-center" style={TRACK(focused())}>
                <Show
                  when={row.level === undefined}
                  fallback={<Ruler value={row.level ?? 0} min={-2} max={2} step={1} width={208} focused={focused()} changed={false} set={() => undefined} />}
                >
                  <span
                    class={`flex h-[37px] items-center rounded-[10px] px-[12px] font-medium whitespace-nowrap ${SMALL}`}
                    style={{ background: i() === 0 && props.changed ? ACCENT : '#fff', color: i() === 0 && props.changed ? '#fff' : '#000' }}
                  >
                    {i() === 0 ? props.film : row.value}
                  </span>
                </Show>
              </div>
            </div>
          );
        }}
      </For>
    </div>
  );
}

interface RecipeState {
  focus: number;
  presented: boolean;
  /** The recipe's film is a card film, edited as itself. */
  card: boolean;
  saved: boolean;
  editor: State;
}

function recipeKey(r: RecipeState, key: string): RecipeState | undefined {
  if (r.presented) {
    const next = keyA(r.editor, key);
    if (next) return { ...r, editor: next };
    if (key === 'Escape' || key === 'Backspace')
      return { ...r, presented: false, saved: r.saved || JSON.stringify(r.editor.film) !== JSON.stringify(r.editor.opened) };
    return undefined;
  }
  if (key === 'ArrowUp' && r.focus > 0) return { ...r, focus: r.focus - 1 };
  if (key === 'ArrowDown' && r.focus < RECIPE_ROWS.length - 1) return { ...r, focus: r.focus + 1 };
  if (key === 's' && r.focus === 0) return { ...r, presented: true };
  return undefined;
}

const CARD_FILM = 'MY FILM 1';

function RecipeScreen(props: { start: RecipeState }) {
  const [r, setR] = createSignal<RecipeState>(props.start);
  const [histogram, setHistogram] = createSignal<number[]>([]);
  onSettled(() => {
    void source().then((src) => setHistogram(src.histogram));
  });
  const filmName = () => (r().card ? CARD_FILM : r().saved ? 'NEW FILM 1' : 'Classic Chrome');
  const spring = 'transform 420ms cubic-bezier(0.22, 1, 0.36, 1), opacity 420ms cubic-bezier(0.22, 1, 0.36, 1)';
  /** The recipe column's right edge peeks past the sheet's. */
  const pushed = () => COLUMN_A + PEEK - RECIPE_WIDTH * 0.92;
  return (
    <div
      tabindex="0"
      class="relative h-[480px] w-[720px] overflow-hidden bg-black font-sans outline-none select-none"
      onKeyDown={(e) => {
        const next = recipeKey(r(), e.key);
        if (next) {
          e.preventDefault();
          setR(next);
        }
      }}
    >
      <Picture film={r().presented || r().saved ? r().editor.film : START} />
      <div
        class="absolute inset-0 origin-left"
        style={{
          transition: spring,
          transform: r().presented ? `translateX(${pushed()}px) translateY(${480 * 0.03}px) scale(0.92)` : 'none',
        }}
      >
        <RecipeColumn focus={r().focus} film={filmName()} changed={r().saved && !r().card} />
        <div
          class="pointer-events-none absolute inset-y-0 left-0 rounded-[10px] bg-black"
          style={{ width: `${RECIPE_WIDTH}px`, opacity: r().presented ? 0.4 : 0, transition: spring }}
        />
      </div>
      <Show when={!r().presented}>
        <Legend hints={r().focus === 0 ? [['▲', 'EDIT FILM'], ['◀▶', 'FILM']] : [['◀▶', 'ADJUST']]} />
      </Show>
      <div
        class="absolute inset-0"
        style={{
          transform: r().presented ? 'none' : `translateX(-${COLUMN_A + 24}px)`,
          'pointer-events': r().presented ? 'auto' : 'none',
          visibility: r().presented ? 'visible' : 'hidden',
          transition: `${spring}, visibility 0s linear ${r().presented ? '0ms' : '420ms'}`,
        }}
      >
        <div class="absolute inset-y-0 left-0" style={{ width: `${COLUMN_A}px`, 'box-shadow': '8px 0 24px rgb(0 0 0 / 0.45)' }} />
        <LayoutA
          s={r().editor}
          set={(editor) => setR({ ...r(), editor })}
          histogram={histogram()}
          title={r().card ? { name: CARD_FILM } : { name: 'NEW FILM', from: 'from Classic Chrome' }}
          opaque
        />
      </div>
    </div>
  );
}

/* ---- Screens and review ----------------------------------------------------------- */

type LayoutId = 'a' | 'b' | 'c' | 'd';
const KEYS: Record<LayoutId, (s: State, key: string) => State | undefined> = {
  a: keyA,
  b: keyB,
  c: keyC,
  d: (s, key) => toolKey(s, 'curve', key),
};

function Screen(props: { layout: LayoutId; start: State }) {
  const [state, setState] = createSignal<State>(props.start);
  const [histogram, setHistogram] = createSignal<number[]>([]);
  onSettled(() => {
    void source().then((src) => setHistogram(src.histogram));
  });
  return (
    <div
      tabindex="0"
      class="relative h-[480px] w-[720px] overflow-hidden bg-black font-sans outline-none select-none"
      onKeyDown={(e) => {
        const next = KEYS[props.layout](state(), e.key);
        if (next) {
          e.preventDefault();
          setState(next);
        }
      }}
    >
      <Picture film={state().film} />
      <Show when={props.layout === 'a'}>
        <LayoutA s={state()} set={setState} histogram={histogram()} />
      </Show>
      <Show when={props.layout === 'b'}>
        <LayoutB s={state()} set={setState} histogram={histogram()} />
      </Show>
      <Show when={props.layout === 'c'}>
        <LayoutC s={state()} set={setState} histogram={histogram()} />
      </Show>
      <Show when={props.layout === 'd'}>
        <LayoutD s={state()} set={setState} histogram={histogram()} />
      </Show>
    </div>
  );
}

const STATES: { id: string; layout: LayoutId; title: string; state: State }[] = [
  { id: 'a-rows', layout: 'a', title: 'A · Rows, one height, with fly-outs', state: initial({ film: EDITED, control: 1 }) },
  { id: 'a-curve', layout: 'a', title: 'A · TONE CURVE fly-out', state: initial({ film: EDITED, open: 'curve', knot: 4 }) },
  { id: 'a-bend', layout: 'a', title: 'A · TONE CURVE fly-out, BEND', state: initial({ film: EDITED, open: 'curve', knot: 4, bend: true }) },
  { id: 'a-primaries', layout: 'a', title: 'A · PRIMARIES fly-out', state: initial({ film: EDITED, open: 'primaries', primary: 1, control: 2 }) },
  { id: 'b-curve', layout: 'b', title: 'B · Sections of any height, TONE CURVE open', state: initial({ film: EDITED, tool: 'curve', open: 'curve', knot: 7 }) },
  { id: 'b-bands', layout: 'b', title: 'B · SAT BY TONE open', state: initial({ film: EDITED, tool: 'bands', open: 'bands', band: 4 }) },
  { id: 'c-curve', layout: 'c', title: 'C · Tool rail, TONE CURVE', state: initial({ film: EDITED, tool: 'curve', open: 'curve', knot: 2 }) },
  { id: 'c-primaries', layout: 'c', title: 'C · Tool rail, PRIMARIES', state: initial({ film: EDITED, tool: 'primaries', open: 'primaries', primary: 0, control: 1 }) },
  { id: 'd-picture', layout: 'd', title: 'D · On the picture: touch a tone, drag it', state: initial({ film: EDITED, knot: 6, picked: { x: 520, y: 150, u: US[6] + 0.01 } }) },
];

const RECIPE_STATES: { id: string; title: string; state: RecipeState }[] = [
  { id: 'r-row', title: 'Recipe editor · FILM SIMULATION row (press S for the ▲ key)', state: { focus: 0, presented: false, card: false, saved: false, editor: initial({ control: 1 }) } },
  { id: 'r-new', title: 'Recipe editor · a built-in film opens as a new film', state: { focus: 0, presented: true, card: false, saved: false, editor: initial({ control: 1 }) } },
  { id: 'r-card', title: 'Recipe editor · a card film opens as itself', state: { focus: 0, presented: true, card: true, saved: false, editor: initial({ film: EDITED, opened: EDITED, control: 1 }) } },
];

function Review() {
  return (
    <div class="min-h-dvh touch-manipulation bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="mb-[6px] text-[22px] font-semibold">Film editor</h1>
      <p class="mb-[8px] max-w-[720px] text-[#999]">Each screen is live: click it, then use the arrow keys, Enter (OK) and Escape (BACK), or drag with the mouse or a finger.</p>
      <For each={RECIPE_STATES}>
        {(entry) => (
          <section id={entry.id} class="mt-[28px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h2>
            <Fit>
              <RecipeScreen start={entry.state} />
            </Fit>
          </section>
        )}
      </For>
      <For each={STATES}>
        {(entry) => (
          <section id={entry.id} class="mt-[28px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h2>
            <Fit>
              <Screen layout={entry.layout} start={entry.state} />
            </Fit>
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
    <div ref={(element) => (host = element)} class="w-full max-w-[720px]" style={{ height: `${480 * scale()}px` }}>
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
