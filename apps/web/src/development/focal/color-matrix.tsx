import { render } from '@solidjs/web';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import './focal.css';

/* COLOR MIX alternatives: the same 3x3 matrix on linear RGB (white kept) and
   2x2 on CbCr, shown four ways over the developed picture. Every view edits
   one shared state, so switching views keeps the look. */

const PHOTO = new URL('../../engine/assets/default-scene.png', import.meta.url).href;
const MARK = '#ff9f0a';

type Primary = { hue: number; sat: number };
type State = { p: [Primary, Primary, Primary]; rot: number; gain: number };
const NAMES = ['RED', 'GREEN', 'BLUE'] as const;
const INKS = ['#ff453a', '#30d158', '#0a84ff'];
const KR = 0.2126;
const KB = 0.0722;
const KG = 1 - KR - KB;

type V3 = [number, number, number];
type M3 = [V3, V3, V3];

const luma = (c: V3) => KR * c[0] + KG * c[1] + KB * c[2];
const chroma = (c: V3): [number, number] => {
  const y = luma(c);
  return [(c[2] - y) / 1.8556, (c[0] - y) / 1.5748];
};
const fromYC = (y: number, cb: number, cr: number): V3 => {
  const r = y + 1.5748 * cr;
  const b = y + 1.8556 * cb;
  return [r, (y - KR * r - KB * b) / KG, b];
};

/** Columns are where red, green and blue go; columns are then scaled so white stays white. */
function matrix(s: State): M3 {
  const cols: V3[] = [0, 1, 2].map((i) => {
    const e: V3 = [0, 0, 0];
    e[i] = 1;
    const y = luma(e);
    const [cb, cr] = chroma(e);
    const a = (s.p[i].hue * Math.PI) / 180;
    const k = 1 + s.p[i].sat / 100;
    return fromYC(y, k * (cb * Math.cos(a) - cr * Math.sin(a)), k * (cb * Math.sin(a) + cr * Math.cos(a)));
  });
  const m: M3 = [
    [cols[0][0], cols[1][0], cols[2][0]],
    [cols[0][1], cols[1][1], cols[2][1]],
    [cols[0][2], cols[1][2], cols[2][2]],
  ];
  const d = solve(m, [1, 1, 1]);
  return m.map((row) => row.map((v, j) => v * d[j]) as V3) as M3;
}

function solve(m: M3, b: V3): V3 {
  const det = (a: M3) =>
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) -
    a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) +
    a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
  const D = det(m);
  return [0, 1, 2].map((j) => det(m.map((row, i) => row.map((v, k) => (k === j ? b[i] : v)) as V3) as M3) / D) as V3;
}

const dec = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const enc = (v: number) => {
  const c = Math.min(1, Math.max(0, v));
  return c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055;
};

/* ---- The picture ------------------------------------------------------------- */

const W = 480;
const H = 320;
let source: Float32Array | null = null;
const [ready, setReady] = createSignal(false);
const img = new Image();
img.src = PHOTO;
img.onload = () => {
  const c = document.createElement('canvas');
  c.width = W;
  c.height = H;
  const g = c.getContext('2d');
  if (!g) return;
  g.drawImage(img, 0, 0, W, H);
  const px = g.getImageData(0, 0, W, H).data;
  source = new Float32Array(W * H * 3);
  for (let i = 0; i < W * H; i++) for (let k = 0; k < 3; k++) source[i * 3 + k] = dec(px[i * 4 + k] / 255);
  setReady(true);
};

function develop(canvas: HTMLCanvasElement, s: State): void {
  if (!source) return;
  const g = canvas.getContext('2d');
  if (!g) return;
  const out = g.createImageData(W, H);
  const m = matrix(s);
  const a = (s.rot * Math.PI) / 180;
  const ca = Math.cos(a) * (1 + s.gain / 100);
  const sa = Math.sin(a) * (1 + s.gain / 100);
  for (let i = 0; i < W * H; i++) {
    const r = source[i * 3];
    const gr = source[i * 3 + 1];
    const b = source[i * 3 + 2];
    const lr = enc(m[0][0] * r + m[0][1] * gr + m[0][2] * b);
    const lg = enc(m[1][0] * r + m[1][1] * gr + m[1][2] * b);
    const lb = enc(m[2][0] * r + m[2][1] * gr + m[2][2] * b);
    const y = luma([lr, lg, lb]);
    const [cb, cr] = chroma([lr, lg, lb]);
    const o = fromYC(y, ca * cb - sa * cr, sa * cb + ca * cr);
    out.data[i * 4] = enc(dec(o[0])) * 255;
    out.data[i * 4 + 1] = enc(dec(o[1])) * 255;
    out.data[i * 4 + 2] = enc(dec(o[2])) * 255;
    out.data[i * 4 + 3] = 255;
  }
  g.putImageData(out, 0, 0);
}

/* ---- Shared pieces ----------------------------------------------------------- */

const NEUTRAL: State = { p: [{ hue: 0, sat: 0 }, { hue: 0, sat: 0 }, { hue: 0, sat: 0 }], rot: 0, gain: 0 };
const START: State = { p: [{ hue: 6, sat: -10 }, { hue: -8, sat: -25 }, { hue: 4, sat: 10 }], rot: 0, gain: 0 };
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const signed = (v: number, digits = 0) => `${v > 0 ? '+' : v < 0 ? '−' : ''}${Math.abs(v).toFixed(digits)}`;

const PLATE = 'linear-gradient(#262626, #1a1a1a)';
const PLATE_EDGE = 'inset 0 1px 0 rgb(255 255 255 / 0.12), inset 0 -1px 0 rgb(0 0 0 / 0.8)';
const WELL = 'linear-gradient(#060606, #121212)';
const WELL_EDGE = 'inset 0 2px 3px rgb(0 0 0 / 0.9), inset 0 -1px 0 rgb(255 255 255 / 0.1)';

function Screen(props: { s: State; children: JSX.Element; width?: number }) {
  let canvas: HTMLCanvasElement | undefined;
  createEffect(
    () => (ready() ? props.s : null),
    (st) => {
      if (canvas && st) develop(canvas, st);
    },
  );
  return (
    <div class="relative aspect-[3/2] w-full overflow-hidden rounded-[6px] bg-black" style={{ 'container-type': 'inline-size' }}>
      <div class="absolute left-0 top-0 origin-top-left" style={{ width: '720px', height: '480px', transform: 'scale(calc(100cqw / 720px))' }}>
        <canvas ref={canvas} width={W} height={H} class="absolute inset-0 size-full" />
        <div
          class="absolute inset-y-0 left-0 flex flex-col text-white"
          style={{ width: `${props.width ?? 330}px`, background: 'rgb(0 0 0 / 0.86)', 'box-shadow': 'inset -1px 0 0 #000, inset -2px 0 0 rgb(255 255 255 / 0.1)' }}
        >
          {props.children}
        </div>
      </div>
    </div>
  );
}

function Title(props: { text: string }) {
  return <div class="px-[18px] pt-[14px] pb-[8px] text-[24px] font-semibold tracking-wide">{props.text}</div>;
}

/** Pointer drag helper: calls move with the pointer position inside el, in the 720-wide screen's units. */
function drag(el: HTMLElement, e: PointerEvent, move: (x: number, y: number) => void): void {
  e.preventDefault();
  el.setPointerCapture(e.pointerId);
  const at = (ev: PointerEvent) => {
    const r = el.getBoundingClientRect();
    const k = el.offsetWidth / r.width;
    move((ev.clientX - r.left) * k, (ev.clientY - r.top) * k);
  };
  at(e);
  const up = () => {
    el.removeEventListener('pointermove', at);
    el.removeEventListener('pointerup', up);
  };
  el.addEventListener('pointermove', at);
  el.addEventListener('pointerup', up);
}

/* ---- A. Vectorscope ---------------------------------------------------------- */

const SCOPE = 280;
const R0 = SCOPE / 2 - 14;
/** Hue is the angle of the chroma vector; distance out is saturation, a pure primary at RING. */
const RING = R0 * 0.72;
const polar = (angle: number, radius: number): [number, number] => [SCOPE / 2 + radius * Math.cos(angle), SCOPE / 2 - radius * Math.sin(angle)];
const hueOf = (c: V3) => {
  const [cb, cr] = chroma(c);
  return Math.atan2(cr, cb);
};
const satOf = (c: V3) => {
  const hi = Math.max(c[0], c[1], c[2]);
  return hi <= 0 ? 0 : (hi - Math.min(c[0], c[1], c[2])) / hi;
};
const HOME = [0, 1, 2].map((i) => hueOf([i === 0 ? 1 : 0, i === 1 ? 1 : 0, i === 2 ? 1 : 0]));
/** Twelve hue marks around the scope, each in its own color. */
const MARKS = Array.from({ length: 12 }, (_, i) => {
  const h = (i / 12) * Math.PI * 2;
  const c = fromYC(0.5, 0.25 * Math.cos(h), 0.25 * Math.sin(h));
  const k = Math.max(c[0], c[1], c[2]);
  return { h, ink: `rgb(${c.map((v) => Math.round((Math.max(0, v) / k) * 255)).join(' ')})` };
});

function Vectorscope(props: { s: State; set: (s: State) => void }) {
  const [focus, setFocus] = createSignal(0);
  let scatter: HTMLCanvasElement | undefined;
  createEffect(
    () => (ready() ? props.s : null),
    (st) => {
      if (!scatter || !source || !st) return;
      const g = scatter.getContext('2d');
      if (!g) return;
      g.clearRect(0, 0, SCOPE, SCOPE);
      const m = matrix(st);
      g.fillStyle = 'rgb(255 255 255 / 0.18)';
      for (let i = 0; i < W * H; i += 37) {
        const c: V3 = [0, 1, 2].map((r) => m[r][0] * source![i * 3] + m[r][1] * source![i * 3 + 1] + m[r][2] * source![i * 3 + 2]) as V3;
        if (luma(c) < 0.002) continue;
        const e = c.map(enc) as V3;
        const [x, yy] = polar(hueOf(e), satOf(e) * RING);
        g.fillRect(x, yy, 1.4, 1.4);
      }
    },
  );
  const where = (i: number, s: State) => polar(HOME[i] + (s.p[i].hue * Math.PI) / 180, RING * (1 + s.p[i].sat / 100));
  const home = (i: number) => where(i, NEUTRAL);
  const moveTo = (i: number, x: number, y: number) => {
    const c = SCOPE / 2;
    let d = ((Math.atan2(-(y - c), x - c) - HOME[i]) * 180) / Math.PI;
    d = ((d + 540) % 360) - 180;
    const sat = clamp(Math.round((Math.hypot(x - c, y - c) / RING - 1) * 100), -50, 50);
    const p = props.s.p.map((q, k) => (k === i ? { hue: clamp(Math.round(d), -30, 30), sat } : q)) as State['p'];
    props.set({ ...props.s, p });
  };
  let box: HTMLDivElement | undefined;
  return (
    <>
      <Title text="COLOR MIX" />
      <div class="flex justify-center">
        <div
          ref={box}
          class="relative touch-none"
          style={{ width: `${SCOPE}px`, height: `${SCOPE}px` }}
          onPointerDown={(e) => {
            if (!box) return;
            const r = box.getBoundingClientRect();
            const k = box.offsetWidth / r.width;
            const x = (e.clientX - r.left) * k;
            const y = (e.clientY - r.top) * k;
            let best = 0;
            let bd = Infinity;
            for (let i = 0; i < 3; i++) {
              const [px, py] = where(i, props.s);
              const d = Math.hypot(px - x, py - y);
              if (d < bd) [bd, best] = [d, i];
            }
            setFocus(best);
            drag(box, e, (px, py) => moveTo(best, px, py));
          }}
        >
          <div class="absolute inset-[14px] rounded-full" style={{ background: 'linear-gradient(#060606, #121212)', 'box-shadow': 'inset 0 2px 4px rgb(0 0 0 / 0.9), 0 0 0 1px rgb(255 255 255 / 0.18)' }} />
          <canvas ref={scatter} width={SCOPE} height={SCOPE} class="absolute inset-0" />
          <svg class="absolute inset-0" width={SCOPE} height={SCOPE}>
            <circle cx={SCOPE / 2} cy={SCOPE / 2} r={RING} fill="none" stroke="rgb(255 255 255 / 0.12)" />
            <For each={MARKS}>{(m) => <circle cx={polar(m.h, R0 - 6)[0]} cy={polar(m.h, R0 - 6)[1]} r="4" fill={m.ink} />}</For>
            <line x1={SCOPE / 2 - 6} y1={SCOPE / 2} x2={SCOPE / 2 + 6} y2={SCOPE / 2} stroke="rgb(255 255 255 / 0.5)" />
            <line x1={SCOPE / 2} y1={SCOPE / 2 - 6} x2={SCOPE / 2} y2={SCOPE / 2 + 6} stroke="rgb(255 255 255 / 0.5)" />
            <polygon points={[0, 1, 2].map((i) => home(i).join(',')).join(' ')} fill="none" stroke="rgb(255 255 255 / 0.3)" stroke-dasharray="4 4" />
            <polygon points={[0, 1, 2].map((i) => where(i, props.s).join(',')).join(' ')} fill="rgb(255 255 255 / 0.06)" stroke="rgb(255 255 255 / 0.8)" stroke-width="1.5" />
            <For each={[0, 1, 2]}>
              {(i) => (
                <g>
                  <circle cx={where(i, props.s)[0]} cy={where(i, props.s)[1]} r={focus() === i ? 15 : 12} fill={INKS[i]} stroke="#fff" stroke-width={focus() === i ? 3 : 1.5} />
                </g>
              )}
            </For>
          </svg>
        </div>
      </div>
      <div class="mx-[14px] mt-[10px] flex items-center justify-between rounded-[10px] px-[14px] py-[10px]" style={{ background: PLATE, 'box-shadow': PLATE_EDGE }}>
        <span class="flex items-center gap-[10px] text-[20px] font-medium">
          <span class="size-[14px] rounded-full" style={{ background: INKS[focus()] }} />
          {NAMES[focus()]}
        </span>
        <span class="text-[18px] text-white/60">
          HUE <b class="text-[22px] text-white">{signed(props.s.p[focus()].hue)}</b>
          <span class="ml-[12px]">SAT</span> <b class="text-[22px] text-white">{signed(props.s.p[focus()].sat)}</b>
        </span>
      </div>
    </>
  );
}

/* ---- B. Broadcast linear matrix ---------------------------------------------- */

const PAIRS: [number, number, string][] = [
  [0, 1, 'R−G'],
  [0, 2, 'R−B'],
  [1, 0, 'G−R'],
  [1, 2, 'G−B'],
  [2, 0, 'B−R'],
  [2, 1, 'B−G'],
];

function LinearMatrix(props: { s: State }) {
  const [focus, setFocus] = createSignal(0);
  return (
    <>
      <Title text="LINEAR MATRIX" />
      <div class="flex flex-col gap-[6px] px-[14px]">
        <For each={PAIRS}>
          {([r, c, name], i) => {
            const v = () => Math.round(matrix(props.s)[r][c] * 100);
            return (
              <div
                class="flex h-[52px] items-center justify-between rounded-[10px] px-[16px]"
                style={{
                  background: focus() === i() ? 'linear-gradient(#5c5c5e, #3d3d3f)' : PLATE,
                  'box-shadow': focus() === i() ? 'inset 0 1px 0 rgb(255 255 255 / 0.35), 0 0 0 1px rgb(255 255 255 / 0.55)' : PLATE_EDGE,
                }}
                onClick={() => setFocus(i())}
              >
                <span class="flex items-center gap-[6px] text-[21px] font-medium">
                  <span class="size-[10px] rounded-full" style={{ background: INKS[r] }} />
                  {name}
                  <span class="size-[10px] rounded-full" style={{ background: INKS[c] }} />
                </span>
                <span class="text-[24px] font-semibold tabular-nums">{signed(v())}</span>
              </div>
            );
          }}
        </For>
      </div>
    </>
  );
}

/* ---- C. RGB mixer ------------------------------------------------------------- */

function Mixer(props: { s: State }) {
  return (
    <>
      <Title text="RGB MIXER" />
      <div class="flex flex-1 justify-between gap-[8px] px-[14px] pb-[16px]">
        <For each={[0, 1, 2]}>
          {(row) => (
            <div class="flex flex-1 flex-col items-center rounded-[10px] pt-[10px] pb-[8px]" style={{ background: PLATE, 'box-shadow': PLATE_EDGE }}>
              <span class="mb-[8px] text-[17px] font-medium" style={{ color: INKS[row] }}>
                {NAMES[row]} OUT
              </span>
              <div class="flex flex-1 gap-[7px]">
                <For each={[0, 1, 2]}>
                  {(col) => {
                    const v = () => matrix(props.s)[row][col];
                    const h = () => clamp(Math.abs(v()) / 1.6, 0, 1) * 100;
                    return (
                      <div class="flex flex-col items-center gap-[6px]">
                        <div class="relative w-[20px] flex-1 rounded-[10px]" style={{ background: WELL, 'box-shadow': WELL_EDGE }}>
                          <div class="absolute inset-x-[4px] h-px bg-white/30" style={{ bottom: '0%' }} />
                          <div
                            class="absolute inset-x-[4px] rounded-[6px]"
                            style={{ bottom: '4px', height: `calc(${h()}% - 8px)`, background: INKS[col], opacity: v() < 0 ? '0.45' : '1', 'min-height': '2px' }}
                          />
                        </div>
                        <span class="text-[14px] tabular-nums text-white/80">{v().toFixed(2)}</span>
                      </div>
                    );
                  }}
                </For>
              </div>
            </div>
          )}
        </For>
      </div>
    </>
  );
}

/* ---- D. COLOR as a wheel (the 2x2) ------------------------------------------- */

function ChromaWheel(props: { s: State; set: (s: State) => void }) {
  let box: HTMLDivElement | undefined;
  const D = 260;
  const c = D / 2;
  const r = () => (D / 2 - 30) * Math.sqrt(1 + props.s.gain / 100) * 0.8;
  const a = () => ((props.s.rot - 90) * Math.PI) / 180;
  return (
    <>
      <Title text="COLOR" />
      <div class="flex justify-center">
        <div
          ref={box}
          class="relative touch-none"
          style={{ width: `${D}px`, height: `${D}px` }}
          onPointerDown={(e) => {
            if (!box) return;
            drag(box, e, (x, y) => {
              const rot = clamp(Math.round((Math.atan2(y - c, x - c) * 180) / Math.PI + 90), -30, 30);
              const k = Math.hypot(x - c, y - c) / ((D / 2 - 30) * 0.8);
              props.set({ ...props.s, rot, gain: clamp(Math.round((k * k - 1) * 100), -100, 100) });
            });
          }}
        >
          <div
            class="absolute inset-[10px] rounded-full"
            style={{ background: 'conic-gradient(from 0deg, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00)', transform: `rotate(${props.s.rot}deg)`, 'mask-image': 'radial-gradient(closest-side, transparent 78%, #000 80%)' }}
          />
          <svg class="absolute inset-0" width={D} height={D}>
            <circle cx={c} cy={c} r={(D / 2 - 30) * 0.8} fill="none" stroke="rgb(255 255 255 / 0.3)" stroke-dasharray="4 4" />
            <circle cx={c} cy={c} r={r()} fill="rgb(255 255 255 / 0.06)" stroke="rgb(255 255 255 / 0.8)" stroke-width="1.5" />
            <line x1={c} y1={c} x2={c + r() * Math.cos(a())} y2={c + r() * Math.sin(a())} stroke="#fff" stroke-width="2" />
            <circle cx={c + r() * Math.cos(a())} cy={c + r() * Math.sin(a())} r="13" fill={MARK} stroke="#fff" stroke-width="2.5" />
          </svg>
        </div>
      </div>
      <div class="mx-[14px] mt-[14px] flex justify-between rounded-[10px] px-[14px] py-[10px] text-[18px] text-white/60" style={{ background: PLATE, 'box-shadow': PLATE_EDGE }}>
        <span>
          HUE <b class="text-[22px] text-white">{signed(props.s.rot)}°</b>
        </span>
        <span>
          SATURATION <b class="text-[22px] text-white">{signed(props.s.gain)}</b>
        </span>
      </div>
    </>
  );
}

/* ---- Page --------------------------------------------------------------------- */

type Ref = { file: string; title: string; note: string; source: string; url: string };
const REFS: Ref[] = [
  { file: 'resolve-rgb-mixer-5.png', title: 'DaVinci Resolve: RGB Mixer', note: 'The 3×3 matrix itself: one group per output channel, one bar per input.', source: 'colorculture.org', url: 'https://colorculture.org/understanding-3x3-matrix-in-color-grading-davinci-resolve/' },
  { file: 'resolve-rgb-mixer-6.png', title: 'DaVinci Resolve: RGB Mixer in use', note: 'A film-print look entered as a matrix.', source: 'colorculture.org', url: 'https://colorculture.org/understanding-3x3-matrix-in-color-grading-davinci-resolve/' },
  { file: 'nuke-colormatrix.png', title: 'Nuke: ColorMatrix', note: 'Nine number fields; output rows, input columns.', source: 'Foundry', url: 'https://learn.foundry.com/nuke/content/reference_guide/color_nodes/colormatrix.html' },
  { file: 'resolve-warper1-lg.jpg', title: 'DaVinci Resolve: Color Warper', note: 'A hue × saturation web over the picture’s colors; drag points. More than a matrix can do.', source: 'Blackmagic Design', url: 'https://www.blackmagicdesign.com/products/davinciresolve/color' },
  { file: 'resolve-chromaticity.jpg', title: 'DaVinci Resolve: chromaticity scope', note: 'The primaries as a triangle, the picture’s colors plotted inside.', source: 'Blackmagic Design', url: 'https://www.blackmagicdesign.com/products/davinciresolve/color' },
  { file: 'resolve-vectorscope.jpg', title: 'DaVinci Resolve: vectorscope', note: 'Hue as angle, saturation as distance from center.', source: 'Blackmagic Design', url: 'https://www.blackmagicdesign.com/products/davinciresolve/color' },
  { file: 'resolve-hue-vs-hue.jpg', title: 'DaVinci Resolve: Hue vs Hue curve', note: 'Hue ranges bent by a curve; finer than a matrix.', source: 'Blackmagic Design', url: 'https://www.blackmagicdesign.com/products/davinciresolve/color' },
  { file: 'resolve-hue-vs-sat.jpg', title: 'DaVinci Resolve: Hue vs Sat curve', note: '', source: 'Blackmagic Design', url: 'https://www.blackmagicdesign.com/products/davinciresolve/color' },
  { file: 'captureone-color-editor-1.jpg', title: 'Capture One: Color Editor', note: 'Pick a color range on a wheel, then hue, saturation and lightness sliders.', source: 'Life After Photoshop', url: 'https://www.lifeafterphotoshop.com/capture-one-color-editor' },
  { file: 'darktable-color-zones-hue.png', title: 'darktable: color zones', note: 'A curve over the hue axis with the picture’s histogram behind it.', source: 'darktable manual', url: 'https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/color-zones/' },
];

function App() {
  const [s, set] = createSignal<State>(START);
  const [mounted, setMounted] = createSignal(false);
  onSettled(() => {
    setMounted(true);
  });
  const views: [string, string, (st: State) => JSX.Element, number?][] = [
    ['Vectorscope', 'Drag a dot: around the circle is hue, out from the center is saturation. The dashed triangle is where the primaries started; dots are the picture’s colors.', (st) => <Vectorscope s={st} set={set} />],
    ['COLOR as a wheel', 'The 2×2: drag the handle around to turn every hue, outward for more saturation.', (st) => <ChromaWheel s={st} set={set} />],
    ['Broadcast linear matrix', 'The six numbers a Sony or Ikegami PAINT menu shows, from the same edit (×100).', (st) => <LinearMatrix s={st} />],
    ['RGB mixer', 'The same matrix as Resolve’s RGB Mixer shows it.', (st) => <Mixer s={st} />],
  ];
  return (
    <div class="mx-auto max-w-[760px] px-[16px] pb-[48px] text-[#ddd]">
      <h1 class="pt-[20px] text-[26px] font-semibold text-white">COLOR MIX alternatives</h1>
      <p class="mt-[6px] text-white/60">Every mock edits the same look. Drag in the vectorscope or the wheel and the other views follow.</p>
      <button type="button" class="mt-[10px] rounded-[8px] px-[12px] py-[6px] text-white" style={{ background: '#333' }} onClick={() => set(NEUTRAL)}>
        Reset
      </button>
      <Show when={mounted()}>
        <For each={views}>
          {([name, note, view]) => (
            <section class="mt-[28px]">
              <h2 class="text-[20px] font-semibold text-white">{name}</h2>
              <p class="mb-[10px] text-white/60">{note}</p>
              <Screen s={s()}>{view(s())}</Screen>
            </section>
          )}
        </For>
      </Show>
      <h1 class="mt-[48px] text-[26px] font-semibold text-white">In other tools</h1>
      <For each={REFS}>
        {(r) => (
          <figure class="mt-[24px]">
            <img src={`refs/${r.file}`} alt={r.title} class="block h-auto w-full rounded-[6px]" />
            <figcaption class="mt-[6px]">
              <b class="text-white">{r.title}</b>
              {r.note ? ` — ${r.note}` : ''}{' '}
              <a class="text-[#a78bfa]" href={r.url}>
                {r.source}
              </a>
            </figcaption>
          </figure>
        )}
      </For>
      <p class="mt-[24px] text-white/60">
        Sony’s manuals describe the PAINT menu’s MATRIX (USER) R‑G…B‑G steps and MULTI MATRIX (16 hue axes, each with hue and saturation) without screen images:{' '}
        <a class="text-[#a78bfa]" href="https://helpguide.sony.net/ilc/2240/v1/en/contents/TP1000669698.html">
          Sony help guide
        </a>
        .
      </p>
    </div>
  );
}

const root = document.getElementById('color-matrix');
if (root) {
  root.textContent = '';
  render(() => <App />, root);
}
