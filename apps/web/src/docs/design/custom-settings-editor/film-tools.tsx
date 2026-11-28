import { For, Show, createEffect, createSignal } from 'solid-js';

import { Artwork, Monogram, stockArtwork } from '../../../design/artwork';
import { Chevron, signed } from '../../../design/controls';
import {
  BODY,
  CAPTION,
  CHOSEN,
  COLUMN,
  DIM,
  EDITED,
  TRACK,
  VALUE,
  WELL,
} from '../../../design/materials';
import {
  CHROMA_SCALES,
  FILM_TOOL_NAMES,
  KNOT_EV,
  MATRIX_SIZE,
  MIX,
  NEUTRAL_KNOTS,
  RAIL,
  SCROLLER,
  US,
  ZONES,
  ZERO_TILT,
  curveEval,
  curveKnots,
  curveTilt,
  disp,
  encode,
  fixedCell,
  fromDisp,
  handlesOf,
  matrixCells,
  mixMatrix,
  mixScales,
  putMatrixCell,
  setCurveData,
  slopesOf,
  type CurveChannel,
  type Film,
  type RgbChannel,
  type UnifiedState,
} from './model';
import { DECODE, source, type Source } from './picture';

/* ---- Film icon & expand icon ----------------------------------------------------- */

const STOCK_FILM_ICONS: Record<string, number> = {
  provia: 0x1dc,
  velvia: 0x1dd,
  astia: 0x1de,
  'classic-chrome': 0x1e5,
  'pro-neg-hi': 0x1df,
  'pro-neg-std': 0x1e0,
  monochrome: 0x1e1,
};

const CARD_MONOGRAMS: Record<string, string> = {
  'leica-x': 'LEI',
  acros: 'AC',
  eterna: 'ET',
  'classic-neg': 'NC',
  'nostalgic-neg': 'NN',
  'reala-ace': 'RA',
};

export function FilmIcon(props: { slug: string; size: number; color: string }) {
  const text = () => (props.slug.startsWith('text:') ? props.slug.slice(5) : undefined);
  const stockId = () => (text() === undefined ? STOCK_FILM_ICONS[props.slug] : undefined);
  const monogram = () => (
    <Monogram
      text={text() ?? CARD_MONOGRAMS[props.slug] ?? props.slug.slice(0, 3).toUpperCase()}
      size={Math.round(props.size * 0.82)}
      color={props.color}
    />
  );
  return (
    <Artwork
      src={stockId() !== undefined ? stockArtwork(stockId() as number) : undefined}
      size={props.size}
      color={props.color}
      fallback={monogram()}
    />
  );
}

export function ExpandIcon(props: { full?: boolean }) {
  return (
    <svg width="20" height="20" viewBox="0 0 22 22">
      <path
        d={
          props.full
            ? 'M8 3v5H3M14 3v5h5M8 19v-5H3M14 19v-5h5'
            : 'M3 8V3h5M19 8V3h-5M3 14v5h5M19 14v5h-5'
        }
        fill="none"
        stroke="#fff"
        stroke-width="2.2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

/* ---- Tone curve with ALL | R | G | B tabs & RGB ghost curves --------------------- */

export const CHANNEL_COLORS: Record<CurveChannel, string> = {
  all: '#ffffff',
  r: '#ff453a',
  g: '#30d158',
  b: '#0a84ff',
};

const CHANNEL_TABS: { id: CurveChannel; label: string }[] = [
  { id: 'all', label: 'ALL' },
  { id: 'r', label: 'R' },
  { id: 'g', label: 'G' },
  { id: 'b', label: 'B' },
];

const NEUTRAL_HANDLES = handlesOf(NEUTRAL_KNOTS, ZERO_TILT);

function pathFromKnots(knots: number[], tilt: number[], w: number, h: number): string {
  const hs = handlesOf(knots, tilt);
  const n = 140;
  let d = '';
  for (let k = 0; k <= n; k++) {
    const u = k / n;
    d += `${k ? ' L' : 'M'}${(u * w).toFixed(1)},${((1 - disp(curveEval(knots, hs, u))) * h).toFixed(1)}`;
  }
  return d;
}

/** Composed ghost path for an RGB channel over the ALL curve. */
function composedGhostPath(f: Film, ch: RgbChannel, w: number, h: number): string {
  const hsAll = handlesOf(f.knots, f.tilt);
  const hsCh = handlesOf(f.channelKnots[ch], f.channelTilt[ch]);
  const n = 140;
  let d = '';
  for (let k = 0; k <= n; k++) {
    const u = k / n;
    const base = curveEval(f.knots, hsAll, u);
    const offset =
      curveEval(f.channelKnots[ch], hsCh, u) - curveEval(NEUTRAL_KNOTS, NEUTRAL_HANDLES, u);
    const y = Math.min(1, Math.max(0, base + offset));
    d += `${k ? ' L' : 'M'}${(u * w).toFixed(1)},${((1 - disp(y)) * h).toFixed(1)}`;
  }
  return d;
}

function channelHasOffset(f: Film, ch: RgbChannel): boolean {
  return (
    f.channelKnots[ch].some((v, i) => Math.abs(v - NEUTRAL_KNOTS[i]) > 1e-4) ||
    f.channelTilt[ch].some((t) => Math.abs(t) > 1e-4)
  );
}

export function CurveChannelTabs(props: {
  channel: CurveChannel;
  film: Film;
  onSelect: (ch: CurveChannel) => void;
}) {
  return (
    <div
      class="flex items-center gap-[2px] rounded-[7px] p-[2px]"
      style={{ background: 'var(--m-well)', 'box-shadow': 'var(--m-well-edge)' }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <For each={CHANNEL_TABS}>
        {(tab) => {
          const active = () => props.channel === tab.id;
          const edited = () => tab.id !== 'all' && channelHasOffset(props.film, tab.id);
          return (
            <button
              type="button"
              class={`flex h-[28px] min-w-[34px] items-center justify-center gap-[4px] rounded-[6px] px-[6px] font-semibold ${CAPTION}`}
              style={active() ? { ...CHOSEN } : { background: 'transparent', color: DIM }}
              onClick={(e) => {
                e.stopPropagation();
                props.onSelect(tab.id);
              }}
            >
              <Show when={tab.id !== 'all'}>
                <span
                  class="size-[6px] shrink-0 rounded-full"
                  style={{
                    background: CHANNEL_COLORS[tab.id],
                    opacity: active() || edited() ? 1 : 0.45,
                  }}
                />
              </Show>
              {tab.label}
            </button>
          );
        }}
      </For>
    </div>
  );
}

export interface CurveProps {
  film: Film;
  opened: Film;
  channel: CurveChannel;
  histogram: number[];
  width: number;
  height: number;
  focus: number;
  bend: boolean;
  set: (film: Film) => void;
  focusKnot: (knot: number) => void;
  active: boolean;
}

export function CurveGraph(props: CurveProps) {
  const pad = 14;
  const w = () => props.width - 2 * pad;
  const h = () => props.height - 2 * pad;
  const X = (u: number) => pad + u * w();
  const Y = (v: number) => pad + (1 - disp(v)) * h();
  const Yh = (d: number) => pad + (1 - d) * h();

  const knots = () => curveKnots(props.film, props.channel);
  const tilt = () => curveTilt(props.film, props.channel);
  const refKnots = () => curveKnots(props.opened, props.channel);
  const refTilt = () => curveTilt(props.opened, props.channel);
  const strokeColor = () => CHANNEL_COLORS[props.channel];

  const changed = (i: number) =>
    Math.abs(knots()[i] - refKnots()[i]) > 1e-4 || Math.abs(tilt()[i] - refTilt()[i]) > 1e-4;

  let drag: { kind: 'knot' | 'handle'; knot: number; y: number; start: number } | undefined;
  let tapStart: { x: number; y: number } | undefined;

  const local = (e: PointerEvent, element: Element) => {
    const box = element.getBoundingClientRect();
    return {
      x: ((e.clientX - box.left) / box.width) * props.width,
      y: ((e.clientY - box.top) / box.height) * props.height,
    };
  };

  const histogramPath = () => {
    const bins = props.histogram;
    if (bins.length === 0) return '';
    let d = `M${X(0)},${Yh(0)}`;
    bins.forEach((v, i) => {
      d += ` L${X((i + 0.5) / bins.length)},${Yh(v * 0.85)}`;
    });
    return `${d} L${X(1)},${Yh(0)} Z`;
  };

  const handlePoints = () => {
    const i = props.focus;
    if (i < 0) return [];
    const k = knots();
    const m = slopesOf(k, tilt())[i];
    const left = i > 0 ? US[i] - (US[i] - US[i - 1]) / 3 : undefined;
    const right = i < 9 ? US[i] + (US[i + 1] - US[i]) / 3 : undefined;
    const at = (u: number) => ({ u, v: k[i] + m * (u - US[i]) });
    return [left, right].filter((u): u is number => u !== undefined).map(at);
  };

  return (
    <svg
      width={props.width}
      height={props.height}
      class="block"
      style={{ 'touch-action': props.active ? 'none' : 'pan-y' }}
      onPointerDown={(e) => {
        const element = e.currentTarget;
        const p = local(e, element);
        if (!props.active) {
          tapStart = { x: e.clientX, y: e.clientY };
          return;
        }
        element.setPointerCapture(e.pointerId);
        if (props.bend) {
          const near = handlePoints().find((q) => Math.hypot(X(q.u) - p.x, Y(q.v) - p.y) < 22);
          if (near) {
            drag = { kind: 'handle', knot: props.focus, y: p.y, start: tilt()[props.focus] };
            return;
          }
        }
        let knot = 0;
        for (let i = 1; i < 10; i++)
          if (Math.abs(X(US[i]) - p.x) < Math.abs(X(US[knot]) - p.x)) knot = i;
        props.focusKnot(knot);
        drag = { kind: 'knot', knot, y: p.y, start: knots()[knot] };
      }}
      onPointerMove={(e) => {
        if (!drag) return;
        const p = local(e, e.currentTarget);
        const dv = (drag.y - p.y) / h();
        if (drag.kind === 'knot') {
          const nextKnots = [...knots()];
          nextKnots[drag.knot] = fromDisp(disp(drag.start) + dv);
          props.set(setCurveData(props.film, props.channel, nextKnots, tilt()));
        } else {
          const nextTilt = [...tilt()];
          const du = (US[Math.min(9, drag.knot + 1)] - US[Math.max(0, drag.knot - 1)]) / 6;
          nextTilt[drag.knot] = drag.start + dv / du;
          props.set(setCurveData(props.film, props.channel, knots(), nextTilt));
        }
      }}
      onPointerUp={(e) => {
        drag = undefined;
        const tap = tapStart && Math.hypot(e.clientX - tapStart.x, e.clientY - tapStart.y) < 8;
        tapStart = undefined;
        if (!tap) return;
        const p = local(e, e.currentTarget);
        let knot = 0;
        for (let i = 1; i < 10; i++)
          if (Math.abs(X(US[i]) - p.x) < Math.abs(X(US[knot]) - p.x)) knot = i;
        props.focusKnot(knot);
      }}
      onPointerCancel={() => {
        drag = undefined;
        tapStart = undefined;
      }}
    >
      <path d={histogramPath()} fill="rgb(255 255 255 / 0.12)" />
      <For each={US}>
        {(u, i) => (
          <line
            x1={X(u)}
            x2={X(u)}
            y1={pad}
            y2={pad + h()}
            stroke={i() === props.focus ? 'rgb(255 255 255 / 0.55)' : 'rgb(255 255 255 / 0.13)'}
            stroke-width={i() === props.focus ? 2 : 1}
          />
        )}
      </For>

      {/* Reference dashed curve */}
      <path
        d={pathFromKnots(refKnots(), refTilt(), w(), h())}
        transform={`translate(${pad},${pad})`}
        fill="none"
        stroke="rgb(255 255 255 / 0.28)"
        stroke-width="1.5"
        stroke-dasharray="4 4"
      />

      {/* Faint colored RGB ghost curves when ALL is active */}
      <Show when={props.channel === 'all'}>
        <For each={['r', 'g', 'b'] as RgbChannel[]}>
          {(ch) => (
            <path
              d={composedGhostPath(props.film, ch, w(), h())}
              transform={`translate(${pad},${pad})`}
              fill="none"
              stroke={CHANNEL_COLORS[ch]}
              stroke-opacity={channelHasOffset(props.film, ch) ? 0.52 : 0.2}
              stroke-width="1.75"
            />
          )}
        </For>
      </Show>

      {/* Faint master ALL curve behind when R/G/B is active */}
      <Show when={props.channel !== 'all'}>
        <path
          d={pathFromKnots(props.film.knots, props.film.tilt, w(), h())}
          transform={`translate(${pad},${pad})`}
          fill="none"
          stroke="rgb(255 255 255 / 0.22)"
          stroke-width="1.5"
        />
      </Show>

      {/* Active editable curve */}
      <path
        d={pathFromKnots(knots(), tilt(), w(), h())}
        transform={`translate(${pad},${pad})`}
        fill="none"
        stroke={strokeColor()}
        stroke-width="3"
      />

      <Show when={props.bend && props.focus >= 0}>
        <For each={handlePoints()}>
          {(q) => (
            <>
              <line
                x1={X(US[props.focus])}
                y1={Y(knots()[props.focus])}
                x2={X(q.u)}
                y2={Y(q.v)}
                stroke={strokeColor()}
                stroke-width="2"
              />
              <circle cx={X(q.u)} cy={Y(q.v)} r="6.5" fill={strokeColor()} />
            </>
          )}
        </For>
      </Show>

      <For each={knots()}>
        {(v, i) => (
          <circle
            cx={X(US[i()])}
            cy={Y(v)}
            r={i() === props.focus ? 9.5 : 6}
            fill={
              changed(i())
                ? props.channel === 'all'
                  ? EDITED
                  : strokeColor()
                : i() === props.focus
                  ? strokeColor()
                  : '#1c1c1e'
            }
            stroke="#fff"
            stroke-width={i() === props.focus ? 2.8 : 1.8}
          />
        )}
      </For>
    </svg>
  );
}

export function knotReadout(f: Film, ch: CurveChannel, knot: number, bend: boolean): string {
  const k = curveKnots(f, ch);
  const t = curveTilt(f, ch);
  const out = Math.round(k[knot] * 100);
  const prefix = ch === 'all' ? '' : `${ch.toUpperCase()} · `;
  const where =
    knot === 0 ? 'BLACK POINT' : knot === 9 ? 'WHITE POINT' : `${ZONES[knot]} ${KNOT_EV[knot]}`;
  return bend
    ? `${prefix}${where} · BEND ${signed(Math.round(t[knot] * 10))}`
    : `${prefix}${where} · ${out}%`;
}

/* ---- Chroma wheel ---------------------------------------------------------------- */

export function ColorWheel(props: { film: Film; set: (film: Film) => void }) {
  const D = 164;
  const c = D / 2;
  const R = 50;
  let box: HTMLDivElement | undefined;
  const radius = () => R * (1 + props.film.saturation / 100);
  const angle = () => ((props.film.hue - 90) * Math.PI) / 180;
  const stretch = () => 1 + props.film.warmth / 100;
  const axisDeg = (Math.atan2(0.8, -0.6) * 180) / Math.PI;
  const set = (e: PointerEvent) => {
    const r = box!.getBoundingClientRect();
    const x = ((e.clientX - r.left) * D) / r.width - c;
    const y = ((e.clientY - r.top) * D) / r.height - c;
    const hue = Math.round((((Math.atan2(y, x) * 180) / Math.PI + 90 + 540) % 360) - 180);
    const saturation =
      Math.round(Math.max(-100, Math.min(100, (Math.hypot(x, y) / R - 1) * 100)) / 5) * 5;
    props.set(CHROMA_SCALES[0].put(CHROMA_SCALES[1].put(props.film, hue), saturation));
  };
  return (
    <div
      ref={(element) => (box = element)}
      class="relative mx-auto my-[4px]"
      style={{ width: `${D}px`, height: `${D}px`, 'touch-action': 'pan-y' }}
    >
      <div
        class="fm-round absolute inset-[6px] rounded-full"
        style={{
          background:
            'conic-gradient(from 0deg, #ff3b30, #ff9500, #ffd60a, #30d158, #00c7be, #0a84ff, #bf5af2, #ff2d78, #ff3b30)',
          transform: `rotate(${props.film.hue}deg)`,
          'mask-image': 'radial-gradient(closest-side, transparent 80%, #000 82%)',
        }}
      />
      <svg class="absolute inset-0" width={D} height={D}>
        <circle
          cx={c}
          cy={c}
          r={R}
          fill="none"
          stroke="rgb(255 255 255 / 0.3)"
          stroke-dasharray="4 4"
        />
        <ellipse
          cx={c}
          cy={c}
          rx={radius() * stretch()}
          ry={radius()}
          transform={`rotate(${axisDeg} ${c} ${c})`}
          fill="rgb(255 255 255 / 0.06)"
          stroke="rgb(255 255 255 / 0.75)"
          stroke-width="1.5"
        />
        <line
          x1={c}
          y1={c}
          x2={c + radius() * Math.cos(angle())}
          y2={c + radius() * Math.sin(angle())}
          stroke="#fff"
          stroke-width="2"
          stroke-linecap="round"
        />
        <circle
          cx={c + radius() * Math.cos(angle())}
          cy={c + radius() * Math.sin(angle())}
          r="10"
          fill="var(--m-mark)"
          stroke="#fff"
          stroke-width="2.5"
        />
      </svg>
      <div
        class="absolute size-[44px] -translate-x-1/2 -translate-y-1/2 touch-none rounded-full"
        style={{
          left: `${c + radius() * Math.cos(angle())}px`,
          top: `${c + radius() * Math.sin(angle())}px`,
        }}
        onPointerDown={(e) => e.currentTarget.setPointerCapture(e.pointerId)}
        onPointerMove={(e) => e.currentTarget.hasPointerCapture(e.pointerId) && set(e)}
      />
    </div>
  );
}

/* ---- Color mix vectorscope & swatch ---------------------------------------------- */

function hsv(h: number, s: number, v: number): [number, number, number] {
  const f = (n: number) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return [f(5), f(3), f(1)];
}

const SCOPE = 224;
const SCOPE_R = SCOPE / 2 - 10;
const RING = SCOPE_R * 0.7;
const PRIMARY_INKS = ['#ff453a', '#30d158', '#0a84ff'];
const scopeAt = (angle: number, radius: number): [number, number] => [
  SCOPE / 2 + radius * Math.cos(angle),
  SCOPE / 2 - radius * Math.sin(angle),
];
const hueAngle = (r: number, g: number, b: number) => {
  const y = 0.299 * r + 0.587 * g + 0.114 * b;
  return Math.atan2(0.713 * (r - y), 0.564 * (b - y));
};
const HOME = [hueAngle(1, 0, 0), hueAngle(0, 1, 0), hueAngle(0, 0, 1)];
const HUE_MARKS = Array.from({ length: 12 }, (_, i) => {
  const [r, g, b] = hsv(i * 30, 1, 1);
  return {
    angle: hueAngle(r, g, b),
    ink: `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`,
  };
});

export function Scope(props: {
  film: Film;
  crop: number;
  chosen: number;
  choose: (p: number) => void;
  set: (film: Film) => void;
}) {
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
        const c = [0, 1, 2].map((k) =>
          encode(Math.max(0, Math.min(1, m[k * 3] * r + m[k * 3 + 1] * gg + m[k * 3 + 2] * b))),
        );
        const hi = Math.max(...c);
        if (hi < 0.04) continue;
        const [x, y] = scopeAt(hueAngle(c[0], c[1], c[2]), ((hi - Math.min(...c)) / hi) * RING);
        g.fillRect(x, y, 1.5, 1.5);
      }
    },
  );
  const handle = (i: number) =>
    scopeAt(
      HOME[i] + (props.film.primaries[i].hue * Math.PI) / 180,
      RING * (1 + props.film.primaries[i].sat / 100),
    );
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
      ref={(element) => (box = element)}
      class="relative mx-auto my-[4px]"
      style={{ width: `${SCOPE}px`, height: `${SCOPE}px`, 'touch-action': 'pan-y' }}
    >
      <div
        class="fm-round absolute inset-[10px] rounded-full"
        style={{
          background: 'var(--m-well)',
          'box-shadow': 'var(--m-well-edge), 0 0 0 1px rgb(255 255 255 / 0.14)',
        }}
      />
      <canvas
        ref={(element) => (dots = element)}
        width={SCOPE}
        height={SCOPE}
        class="absolute inset-0"
      />
      <svg class="absolute inset-0" width={SCOPE} height={SCOPE}>
        <circle
          cx={SCOPE / 2}
          cy={SCOPE / 2}
          r={RING}
          fill="none"
          stroke="rgb(255 255 255 / 0.12)"
        />
        <For each={HUE_MARKS}>
          {(m) => (
            <circle
              cx={scopeAt(m.angle, SCOPE_R - 6)[0]}
              cy={scopeAt(m.angle, SCOPE_R - 6)[1]}
              r="3.5"
              fill={m.ink}
            />
          )}
        </For>
        <polygon
          points={HOME.map((a) => scopeAt(a, RING).join(',')).join(' ')}
          fill="none"
          stroke="rgb(255 255 255 / 0.3)"
          stroke-dasharray="4 4"
        />
        <polygon
          points={[0, 1, 2].map((i) => handle(i).join(',')).join(' ')}
          fill="rgb(255 255 255 / 0.05)"
          stroke="rgb(255 255 255 / 0.75)"
          stroke-width="1.5"
        />
        <For each={[0, 1, 2]}>
          {(i) => (
            <circle
              cx={handle(i)[0]}
              cy={handle(i)[1]}
              r={props.chosen === i ? 12 : 9.5}
              fill={PRIMARY_INKS[i]}
              stroke="#fff"
              stroke-width={props.chosen === i ? 3 : 1.5}
            />
          )}
        </For>
      </svg>
      <For each={[0, 1, 2]}>
        {(i) => (
          <div
            class="absolute size-[44px] -translate-x-1/2 -translate-y-1/2 touch-none rounded-full"
            style={{ left: `${handle(i)[0]}px`, top: `${handle(i)[1]}px` }}
            onPointerDown={(e) => {
              props.choose(i);
              e.currentTarget.setPointerCapture(e.pointerId);
            }}
            onPointerMove={(e) => {
              if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
              const [x, y] = local(e);
              moveTo(i, x, y);
            }}
          />
        )}
      </For>
    </div>
  );
}

export function Swatch(props: { film: Film; mix: number }) {
  const after = () => {
    const m = mixMatrix(props.film);
    const l = MIX[props.mix].linear;
    const rgb = [0, 1, 2].map(
      (r) =>
        encode(
          Math.max(0, Math.min(1, m[r * 3] * l[0] + m[r * 3 + 1] * l[1] + m[r * 3 + 2] * l[2])),
        ) * 255,
    );
    return `rgb(${rgb.map(Math.round).join(' ')})`;
  };
  return (
    <span
      class="fm-round block size-[18px] shrink-0 rounded-full"
      style={{
        background: `linear-gradient(90deg, ${MIX[props.mix].color} 50%, ${after()} 50%)`,
        'box-shadow': 'inset 0 0 0 2px rgb(255 255 255 / 0.35)',
      }}
    />
  );
}

/* ---- Luma Sat 5-band graph ------------------------------------------------------- */

const BAND_CENTERS = (f: Film) => {
  const b = [0, ...f.edges, 255];
  return [0, 1, 2, 3, 4].map((i) => (b[i] + b[i + 1]) / 2);
};

export function ToneGraph(props: {
  film: Film;
  opened: Film;
  width: number;
  height: number;
  band: number;
  inside: boolean;
  set: (film: Film) => void;
  focusBand: (band: number) => void;
}) {
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
  let tapStart: { x: number; y: number } | undefined;
  const bandAt = (x: number) => Math.max(0, Math.min(4, Math.floor(x / (cw() + gap))));
  const local = (e: PointerEvent, element: Element) => {
    const box = element.getBoundingClientRect();
    return {
      x: ((e.clientX - box.left) / box.width) * props.width,
      y: ((e.clientY - box.top) / box.height) * props.height,
    };
  };
  return (
    <svg
      width={props.width}
      height={props.height}
      class="block"
      style={{ 'touch-action': props.inside ? 'none' : 'pan-y' }}
      onPointerDown={(e) => {
        const element = e.currentTarget;
        if (!props.inside) {
          tapStart = { x: e.clientX, y: e.clientY };
          return;
        }
        element.setPointerCapture(e.pointerId);
        const p = local(e, element);
        const band = bandAt(p.x);
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
      onPointerUp={(e) => {
        drag = undefined;
        const tap = tapStart && Math.hypot(e.clientX - tapStart.x, e.clientY - tapStart.y) < 8;
        tapStart = undefined;
        if (tap) props.focusBand(bandAt(local(e, e.currentTarget).x));
      }}
      onPointerCancel={() => {
        drag = undefined;
        tapStart = undefined;
      }}
    >
      <For each={[0, 1, 2, 3, 4]}>
        {(i) => {
          const x = () => i * (cw() + gap);
          const v = () => Math.round(level(i) * 255 * 0.8);
          const pct = () => props.film.bands[i];
          const chosen = () => props.inside && props.band === i;
          return (
            <>
              <rect
                x={x()}
                y="0"
                width={cw()}
                height={props.height}
                rx="8"
                fill={`rgb(${v()} ${v()} ${v()})`}
              />
              <line
                x1={x() + 8}
                x2={x() + cw() - 8}
                y1={Y(0)}
                y2={Y(0)}
                stroke="rgb(255 255 255 / 0.5)"
                stroke-dasharray="4 4"
              />
              <line
                x1={x() + cw() / 2}
                x2={x() + cw() / 2}
                y1={Y(0)}
                y2={Y(pct())}
                stroke={pct() !== props.opened.bands[i] ? EDITED : '#fff'}
                stroke-width="4"
                stroke-linecap="round"
              />
              <circle
                cx={x() + cw() / 2}
                cy={Y(pct())}
                r={chosen() ? 14 : 11}
                fill={sample(i)}
                stroke={chosen() ? '#fff' : 'rgb(255 255 255 / 0.7)'}
                stroke-width={chosen() ? 3.5 : 2}
              />
            </>
          );
        }}
      </For>
    </svg>
  );
}

/* ---- Matrix numbers sheet (2x2 Chroma / 3x3 Color Mix) --------------------------- */

export function NumbersSheet(props: { s: UnifiedState; update: (s: UnifiedState) => void }) {
  const n = () => props.s.numbers!;
  const size = () => MATRIX_SIZE[n().tool];
  const labels = () => (n().tool === 'mix' ? ['R', 'G', 'B'] : ['Cb', 'Cr']);
  const values = () => matrixCells(props.s.film, n().tool);
  return (
    <div
      class="absolute inset-y-0 z-20 flex flex-col px-[12px] pt-[10px]"
      style={{ ...COLUMN, left: `${RAIL}px`, width: `${SCROLLER}px` }}
    >
      <div class="mb-[10px] flex items-center gap-[6px]">
        <button
          type="button"
          class="flex size-[32px] items-center justify-center text-white/70"
          onClick={() => props.update({ ...props.s, numbers: null })}
        >
          <Chevron dir="left" size={22} />
        </button>
        <span class={`font-semibold ${BODY}`}>{FILM_TOOL_NAMES[n().tool]}</span>
        <span class={`ml-auto ${CAPTION}`} style={{ color: DIM }}>
          NUMBERS
        </span>
      </div>
      <div
        class="grid gap-[8px]"
        style={{ 'grid-template-columns': `30px repeat(${size()}, 1fr)` }}
      >
        <span />
        <For each={labels()}>
          {(l) => (
            <span class={`text-center ${CAPTION}`} style={{ color: DIM }}>
              {l}
            </span>
          )}
        </For>
        <For each={labels()}>
          {(rowLabel, r) => (
            <>
              <span class={`flex items-center ${CAPTION}`} style={{ color: DIM }}>
                {rowLabel}
              </span>
              <For each={labels()}>
                {(_, col) => {
                  const cell = () => r() * size() + col();
                  const fixed = () => fixedCell(n().tool, cell());
                  const on = () => n().cell === cell();
                  return (
                    <div
                      class={`flex h-[52px] touch-none items-center justify-center rounded-[8px] ${VALUE}`}
                      style={
                        fixed()
                          ? {
                              color: 'rgb(255 255 255 / 0.45)',
                              'box-shadow': 'inset 0 0 0 1px rgb(255 255 255 / 0.14)',
                            }
                          : on()
                            ? { ...TRACK(true), color: '#fff' }
                            : { ...WELL, color: '#fff' }
                      }
                      onPointerDown={(e) => {
                        if (fixed()) return;
                        const y0 = e.clientY;
                        const v0 = values()[cell()];
                        const at = cell();
                        props.update({ ...props.s, numbers: { ...n(), cell: at } });
                        e.currentTarget.setPointerCapture(e.pointerId);
                        const el = e.currentTarget;
                        const move = (ev: PointerEvent) =>
                          props.update({
                            ...props.s,
                            film: putMatrixCell(
                              props.s.film,
                              n().tool,
                              at,
                              v0 + (y0 - ev.clientY) / 300,
                            ),
                          });
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
