import { render } from '@solidjs/web';
import { For, Show, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import './focal.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Fly-outs (WB SHIFT, film simulation notes) extend
   the focused row across the right column. Compare keeps a snapshot recipe
   and shows it beside the current one. The same screen serves RAW CONVERSION
   in playback. */

/* ---- Recipe model ------------------------------------------------------------ */

const FILMS = [
  { name: 'PROVIA', long: 'PROVIA/STANDARD', note: 'Standard reproduction, suitable for a wide range of subjects.' },
  { name: 'VELVIA', long: 'Velvia/VIVID', note: 'High contrast with vivid color for landscapes and nature.' },
  { name: 'ASTIA', long: 'ASTIA/SOFT', note: 'Soft tones with gentle skin color for portraits.' },
  { name: 'CLASSIC CHROME', long: 'CLASSIC CHROME', note: 'Subdued color and stronger shadow contrast for a documentary look.' },
  { name: 'REALA ACE', long: 'REALA ACE', note: 'Faithful color with crisp tonality.' },
  { name: 'CLASSIC Neg.', long: 'CLASSIC Neg.', note: 'Hard tonality with distinct color, like a snapshot on negative film.' },
  { name: 'ETERNA', long: 'ETERNA/CINEMA', note: 'Muted color and rich shadows, for movies.' },
  { name: 'ACROS', long: 'ACROS', note: 'Monochrome with smooth gradation and fine sharpness.' },
] as const;

const WB = ['AUTO', 'AUTO WHITE', 'DAYLIGHT', 'SHADE', 'FLUOR. 1', 'INCAND.', '5600K'] as const;
const EFFECT = ['OFF', 'WEAK', 'STRONG'] as const;

type Icon = 'film' | 'wb' | 'grain' | 'chrome' | 'fxblue' | 'highlight' | 'shadow' | 'color' | 'sharp' | 'nr' | 'clarity' | 'dr';

interface SliderSetting {
  kind: 'slider';
  id: string;
  name: string;
  icon: Icon;
  min: number;
  max: number;
  step: number;
}
interface ChoiceSetting {
  kind: 'choice';
  id: string;
  name: string;
  icon: Icon;
  choices: readonly string[];
}
type Setting = SliderSetting | ChoiceSetting;

const SETTINGS: Setting[] = [
  { kind: 'choice', id: 'film', name: 'FILM SIMULATION', icon: 'film', choices: FILMS.map((f) => f.name) },
  { kind: 'choice', id: 'wb', name: 'WHITE BALANCE', icon: 'wb', choices: WB },
  { kind: 'choice', id: 'dr', name: 'DYNAMIC RANGE', icon: 'dr', choices: ['AUTO', 'DR100', 'DR200', 'DR400'] },
  { kind: 'slider', id: 'highlight', name: 'HIGHLIGHT', icon: 'highlight', min: -2, max: 4, step: 0.5 },
  { kind: 'slider', id: 'shadow', name: 'SHADOW', icon: 'shadow', min: -2, max: 4, step: 0.5 },
  { kind: 'slider', id: 'color', name: 'COLOR', icon: 'color', min: -4, max: 4, step: 1 },
  { kind: 'choice', id: 'chrome', name: 'COLOR CHROME EFFECT', icon: 'chrome', choices: EFFECT },
  { kind: 'choice', id: 'fxblue', name: 'COLOR CHROME FX BLUE', icon: 'fxblue', choices: EFFECT },
  { kind: 'choice', id: 'grain', name: 'GRAIN EFFECT', icon: 'grain', choices: ['OFF', 'WEAK SMALL', 'WEAK LARGE', 'STRONG SMALL', 'STRONG LARGE'] },
  { kind: 'slider', id: 'clarity', name: 'CLARITY', icon: 'clarity', min: -5, max: 5, step: 1 },
  { kind: 'slider', id: 'sharp', name: 'SHARPNESS', icon: 'sharp', min: -4, max: 4, step: 1 },
  { kind: 'slider', id: 'nr', name: 'HIGH ISO NR', icon: 'nr', min: -4, max: 4, step: 1 },
];

/** Slider settings hold a number, choice settings the index of their choice. */
type Recipe = Record<string, number> & { shiftR: number; shiftB: number };

const BASE: Recipe = {
  film: 3, wb: 0, dr: 0, highlight: -1, shadow: 1, color: 2, chrome: 1, fxblue: 0,
  grain: 1, clarity: 0, sharp: -1, nr: -4, shiftR: 2, shiftB: -3,
};

const fmt = (n: number) => (n > 0 ? `+${n}` : `${n}`);

function valueText(setting: Setting, recipe: Recipe) {
  const v = recipe[setting.id];
  return setting.kind === 'slider' ? fmt(v) : setting.choices[v];
}

/* ---- Icons ------------------------------------------------------------------- */

function SettingIcon(props: { icon: Icon }) {
  const s = { fill: 'none', stroke: 'currentColor', 'stroke-width': 2.4, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' } as const;
  const shapes: Record<Icon, JSX.Element> = {
    film: (
      <>
        <rect x="5" y="4" width="22" height="24" rx="2" {...s} />
        <path d="M10 4v24M22 4v24M5 10h5M5 16h5M5 22h5M22 10h5M22 16h5M22 22h5" {...s} stroke-width="1.8" />
      </>
    ),
    wb: (
      <>
        <text x="16" y="21" text-anchor="middle" font-size="14" font-weight="800" fill="currentColor">WB</text>
        <rect x="3" y="6" width="26" height="20" rx="3" {...s} />
      </>
    ),
    dr: (
      <>
        <text x="16" y="21" text-anchor="middle" font-size="13" font-weight="800" fill="currentColor">DR</text>
        <path d="M4 26h24M4 6h24" {...s} />
      </>
    ),
    highlight: (
      <>
        <circle cx="16" cy="16" r="10" {...s} />
        <path d="M16 6a10 10 0 0 1 0 20z" fill="currentColor" />
        <path d="M2 4l5 3M30 4l-5 3" {...s} stroke-width="1.8" />
      </>
    ),
    shadow: (
      <>
        <circle cx="16" cy="16" r="10" {...s} />
        <path d="M16 6a10 10 0 0 0 0 20z" fill="currentColor" />
        <path d="M2 28l5-3M30 28l-5-3" {...s} stroke-width="1.8" />
      </>
    ),
    color: (
      <>
        <circle cx="12" cy="13" r="7" {...s} />
        <circle cx="20" cy="13" r="7" {...s} />
        <circle cx="16" cy="20" r="7" {...s} />
      </>
    ),
    chrome: (
      <>
        <path d="M16 4c5 7 8 11 8 15a8 8 0 0 1-16 0c0-4 3-8 8-15z" {...s} />
        <path d="M12 20a4 4 0 0 0 4 4" {...s} />
      </>
    ),
    fxblue: (
      <>
        <path d="M16 4c5 7 8 11 8 15a8 8 0 0 1-16 0c0-4 3-8 8-15z" {...s} />
        <text x="16" y="23" text-anchor="middle" font-size="9" font-weight="800" fill="currentColor">B</text>
      </>
    ),
    grain: (
      <>
        <rect x="4" y="4" width="24" height="24" rx="3" {...s} />
        <For each={[[10, 10], [17, 9], [23, 12], [12, 17], [20, 18], [9, 23], [16, 23], [24, 24]]}>
          {([x, y]) => <circle cx={x} cy={y} r="1.6" fill="currentColor" />}
        </For>
      </>
    ),
    clarity: (
      <>
        <path d="M16 3l11 13-11 13L5 16z" {...s} />
        <path d="M16 3v26" {...s} />
      </>
    ),
    sharp: <path d="M16 4l12 22H4z" {...s} />,
    nr: (
      <>
        <text x="16" y="21" text-anchor="middle" font-size="12" font-weight="800" fill="currentColor">NR</text>
        <rect x="3" y="6" width="26" height="20" rx="10" {...s} />
      </>
    ),
  };
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" class="shrink-0 text-label">
      {shapes[props.icon]}
    </svg>
  );
}

function Arrow(props: { dir: 'left' | 'right'; dim?: boolean }) {
  return (
    <svg width="10" height="16" viewBox="0 0 10 16" class={props.dim ? 'text-dim' : 'text-label'}>
      <path
        d={props.dir === 'left' ? 'M8 2L2 8l6 6' : 'M2 2l6 6-6 6'}
        fill="none"
        stroke="currentColor"
        stroke-width="2.6"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

/* ---- Live view --------------------------------------------------------------- */

/** An approximation of the recipe's look as CSS filters, enough to see edits move. */
function look(r: Recipe): string {
  const film = FILMS[r.film].name;
  const mono = film === 'ACROS';
  const sat = { PROVIA: 1, VELVIA: 1.45, ASTIA: 0.95, 'CLASSIC CHROME': 0.72, 'REALA ACE': 1.02, 'CLASSIC Neg.': 0.85, ETERNA: 0.6, ACROS: 0 }[film];
  const contrast = 1 + (r.shadow + r.highlight) * 0.06 + (film === 'CLASSIC Neg.' ? 0.12 : 0) + r.clarity * 0.02;
  const warm = r.shiftR * 2 - r.shiftB * 2 + ([0, -4, 0, 10, -14, -24, 2][r.wb] ?? 0);
  return [
    `saturate(${mono ? 0 : sat * (1 + r.color * 0.12) * (1 + r.chrome * 0.04)})`,
    `contrast(${contrast})`,
    `brightness(${1 - r.shadow * 0.025 + 0.02})`,
    `sepia(${Math.max(0, warm) / 60})`,
    `hue-rotate(${Math.min(0, warm) * 1.2}deg)`,
  ].join(' ');
}

/** A synthetic street scene drawn in SVG; it stands in for the sensor image. */
function Scene(props: { recipe: Recipe; class?: string }) {
  return (
    <div class={`absolute inset-0 overflow-hidden ${props.class ?? ''}`} style={{ filter: look(props.recipe) }}>
      <svg viewBox="0 0 720 480" preserveAspectRatio="xMidYMid slice" class="h-full w-full">
        <defs>
          <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#5d8fd0" />
            <stop offset="0.6" stop-color="#f3c391" />
            <stop offset="1" stop-color="#f7a166" />
          </linearGradient>
          <linearGradient id="street" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#5a4c48" />
            <stop offset="1" stop-color="#1b1716" />
          </linearGradient>
          <radialGradient id="sun" cx="0.72" cy="0.48" r="0.18">
            <stop offset="0" stop-color="#fff6d8" />
            <stop offset="1" stop-color="#fff6d8" stop-opacity="0" />
          </radialGradient>
        </defs>
        <rect width="720" height="480" fill="url(#sky)" />
        <rect width="720" height="480" fill="url(#sun)" />
        <path d="M0 140h70v-30h40v30h30v240H0z" fill="#2f3a4a" />
        <path d="M150 90h90v290h-90z" fill="#7a4a3a" />
        <path d="M250 170h70v210h-70z" fill="#c8a27a" />
        <path d="M600 120h120v260H600z" fill="#3c2e2c" />
        <path d="M560 200h50v180h-50z" fill="#8c5c3e" />
        <For each={[0, 1, 2, 3, 4, 5]}>
          {(i) => (
            <>
              <rect x={162} y={110 + i * 42} width="22" height="26" fill="#f2d48a" opacity={i % 2 ? 0.9 : 0.35} />
              <rect x={204} y={110 + i * 42} width="22" height="26" fill="#f2d48a" opacity={i % 3 ? 0.4 : 0.95} />
              <rect x={616} y={140 + i * 38} width="18" height="22" fill="#e9b765" opacity={i % 2 ? 0.3 : 0.85} />
              <rect x={660} y={140 + i * 38} width="18" height="22" fill="#e9b765" opacity={i % 2 ? 0.85 : 0.3} />
            </>
          )}
        </For>
        <path d="M0 380h720v100H0z" fill="url(#street)" />
        <path d="M330 380l40 100h-20z" fill="#e8e2d2" opacity="0.5" />
        <rect x="430" y="330" width="120" height="50" rx="14" fill="#c62f2a" />
        <rect x="446" y="314" width="78" height="30" rx="10" fill="#8a1f1c" />
        <circle cx="458" cy="384" r="12" fill="#111" />
        <circle cx="526" cy="384" r="12" fill="#111" />
        <rect x="372" y="300" width="12" height="80" rx="6" fill="#1f2b37" />
        <circle cx="378" cy="290" r="12" fill="#e2b393" />
        <path d="M80 210l6 170h-12z" fill="#222" />
        <circle cx="80" cy="206" r="9" fill="#ffd27a" />
      </svg>
    </div>
  );
}

/* ---- Rows -------------------------------------------------------------------- */

const COLUMN = 312;
const ROW = 50;

function Ticks(props: { setting: SliderSetting; value: number; compare?: number; focused: boolean }) {
  const steps = () => Math.round((props.setting.max - props.setting.min) / props.setting.step) + 1;
  const at = (v: number) => ((v - props.setting.min) / (props.setting.max - props.setting.min)) * 100;
  return (
    <div class="relative h-[24px] flex-1">
      <div class="absolute inset-x-0 top-1/2 h-[2px] -translate-y-1/2 bg-white/18" />
      <For each={Array.from({ length: steps() }, (_, i) => i)}>
        {(i) => {
          const v = props.setting.min + i * props.setting.step;
          const whole = Number.isInteger(v);
          return (
            <div
              class="absolute top-1/2 w-[2px] -translate-x-1/2 -translate-y-1/2 rounded-full"
              style={{
                left: `${at(v)}%`,
                height: v === 0 ? '20px' : whole ? '10px' : '5px',
                background: v === 0 ? 'rgb(255 255 255 / 0.75)' : 'rgb(255 255 255 / 0.32)',
              }}
            />
          );
        }}
      </For>
      <div
        class="absolute top-1/2 h-[2px] -translate-y-1/2 bg-(--accent)"
        style={{
          left: `${Math.min(at(0), at(props.value))}%`,
          width: `${Math.abs(at(props.value) - at(0))}%`,
        }}
      />
      <Show when={props.compare !== undefined && props.compare !== props.value}>
        <div
          class="absolute top-1/2 h-[14px] w-[14px] -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white/60"
          style={{ left: `${at(props.compare!)}%` }}
        />
      </Show>
      <div
        class={`absolute top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white shadow-[0_1px_4px_rgb(0_0_0/0.6)] ${
          props.focused ? 'h-[18px] w-[18px]' : 'h-[12px] w-[12px]'
        }`}
        style={{ left: `${at(props.value)}%` }}
      />
    </div>
  );
}

function Carousel(props: { setting: ChoiceSetting; value: number; focused: boolean; changed: boolean }) {
  const n = () => props.setting.choices.length;
  return (
    <div class="flex min-w-0 flex-1 items-center gap-[8px]">
      <Show when={props.focused}>
        <Arrow dir="left" dim={props.value === 0} />
      </Show>
      <span class="min-w-0 flex-1 truncate text-center text-[22px] leading-none font-semibold uppercase tracking-[0.01em]">
        {props.setting.choices[props.value]}
      </span>
      <Show when={props.focused}>
        <Arrow dir="right" dim={props.value === n() - 1} />
      </Show>
      <Show when={!props.focused}>
        <span class="w-[10px]" />
      </Show>
      <Show when={props.changed}>
        <span class="absolute right-[8px] top-[8px] h-[7px] w-[7px] rounded-full bg-(--accent)" />
      </Show>
    </div>
  );
}

function SettingRow(props: {
  setting: Setting;
  recipe: Recipe;
  compare?: Recipe;
  focused: boolean;
  extended?: boolean;
}) {
  const changed = () => props.compare !== undefined && props.compare[props.setting.id] !== props.recipe[props.setting.id];
  return (
    <div
      data-focused={props.focused ? '' : undefined}
      class={`relative flex items-center gap-[12px] pr-[14px] pl-[16px] ${props.focused ? 'focal-focused' : ''} ${
        props.extended ? 'rounded-r-none' : ''
      }`}
      style={{
        height: `${ROW}px`,
        background: props.focused ? 'rgb(58 58 60 / 0.92)' : undefined,
        'box-shadow': props.focused ? undefined : 'inset 0 -1px 0 rgb(255 255 255 / 0.07)',
      }}
    >
      <SettingIcon icon={props.setting.icon} />
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <Carousel
            setting={props.setting as ChoiceSetting}
            value={props.recipe[props.setting.id]}
            focused={props.focused}
            changed={changed()}
          />
        }
      >
        {(slider) => (
          <>
            <Ticks
              setting={slider()}
              value={props.recipe[slider().id]}
              compare={props.compare?.[slider().id]}
              focused={props.focused}
            />
            <span class="w-[46px] text-right text-[22px] leading-none font-semibold tabular-nums">
              {fmt(props.recipe[slider().id])}
            </span>
          </>
        )}
      </Show>
    </div>
  );
}

/* ---- Fly-outs ---------------------------------------------------------------- */

/** WB SHIFT: R on the horizontal axis, B on the vertical, -9 to +9 as stock. */
function ShiftGrid(props: { r: number; b: number }) {
  const cell = 13;
  const size = cell * 19;
  const pos = (v: number) => (v + 9) * cell;
  return (
    <div class="flex flex-col items-center gap-[24px]">
      <div class="relative" style={{ width: `${size}px`, height: `${size}px` }}>
        <div
          class="absolute inset-0 rounded-[6px]"
          style={{
            background:
              'linear-gradient(to right, rgb(60 200 120 / 0.55), transparent 50%, rgb(240 70 80 / 0.55)), linear-gradient(to top, rgb(240 200 60 / 0.55), transparent 50%, rgb(60 110 255 / 0.55))',
          }}
        />
        <svg class="absolute inset-0" width={size} height={size}>
          <For each={Array.from({ length: 20 }, (_, i) => i)}>
            {(i) => (
              <>
                <line x1={i * cell} y1="0" x2={i * cell} y2={size} stroke="rgb(255 255 255 / 0.12)" />
                <line x1="0" y1={i * cell} x2={size} y2={i * cell} stroke="rgb(255 255 255 / 0.12)" />
              </>
            )}
          </For>
          <line x1={size / 2} y1="0" x2={size / 2} y2={size} stroke="rgb(255 255 255 / 0.5)" />
          <line x1="0" y1={size / 2} x2={size} y2={size / 2} stroke="rgb(255 255 255 / 0.5)" />
        </svg>
        <div
          class="absolute rounded-[3px] border-[3px] border-white bg-(--accent) shadow-[0_1px_6px_rgb(0_0_0/0.7)]"
          style={{ left: `${pos(props.r) - 3}px`, top: `${pos(-props.b) - 3}px`, width: `${cell + 6}px`, height: `${cell + 6}px` }}
        />
        <span class="absolute -top-[2px] left-1/2 -translate-x-1/2 -translate-y-full text-[16px] font-bold">B</span>
        <span class="absolute top-1/2 -right-[4px] translate-x-full -translate-y-1/2 text-[16px] font-bold">R</span>
        <span class="absolute -bottom-[2px] left-1/2 -translate-x-1/2 translate-y-full text-[16px] font-bold">Y</span>
        <span class="absolute top-1/2 -left-[4px] -translate-x-full -translate-y-1/2 text-[16px] font-bold">G</span>
      </div>
    </div>
  );
}

/** Extends the focused row across the right column, then drops a panel below it. */
function Flyout(props: { top: number; title: string; value?: string; children: JSX.Element; height: number }) {
  const top = () => Math.max(56, Math.min(480 - props.height - 44, props.top));
  return (
    <>
      <div
        class="absolute bg-[rgb(58_58_60/0.92)]"
        style={{ left: `${COLUMN}px`, top: `${props.top}px`, width: '12px', height: `${ROW}px` }}
      />
      <div
        class="absolute flex flex-col rounded-[14px] bg-[rgb(30_30_32/0.88)] p-[18px] backdrop-blur-[18px] shadow-[0_8px_30px_rgb(0_0_0/0.5)]"
        style={{ left: `${COLUMN + 12}px`, right: '12px', top: `${top()}px`, height: `${props.height}px` }}
      >
        <div class="mb-[14px] flex items-baseline justify-between text-[20px] font-bold tracking-[0.04em] uppercase">
          <span class="text-secondary">{props.title}</span>
          <span class="tabular-nums">{props.value}</span>
        </div>
        {props.children}
      </div>
    </>
  );
}

/* ---- The editor screen ------------------------------------------------------- */

type Fly = 'none' | 'shift' | 'film';
type CompareView = 'off' | 'split' | 'diff';
type Mode = 'live' | 'raw';

interface EditorState {
  mode: Mode;
  recipe: Recipe;
  focus: number;
  fly: Fly;
  /** The snapshot recipe kept for comparing, with the frame it was taken on. */
  snapshot?: Recipe;
  compare: CompareView;
  name: string;
}

function Hint(props: { keys: string; label: string }) {
  return (
    <span class="flex items-center gap-[6px]">
      <span class="inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-[4px] bg-white px-[5px] text-[14px] font-bold text-black">
        {props.keys}
      </span>
      <span class="text-[16px] font-semibold tracking-[0.02em] uppercase">{props.label}</span>
    </span>
  );
}

function Editor(props: { state: EditorState }) {
  let list: HTMLDivElement | undefined;
  const [scroll, setScroll] = createSignal(0);
  const s = () => props.state;
  const focused = () => SETTINGS[s().focus];
  const header = 52;
  const rowTop = () => header + s().focus * ROW - scroll();

  onSettled(() => {
    const element = list;
    if (!element) return;
    const follow = () => {
      const top = s().focus * ROW;
      let next = element.scrollTop;
      if (top + ROW * 1.5 > next + element.clientHeight) next = top + ROW * 1.5 - element.clientHeight;
      if (top - ROW / 2 < next) next = Math.max(0, top - ROW / 2);
      element.scrollTop = next;
      setScroll(element.scrollTop);
    };
    const observer = new MutationObserver(follow);
    observer.observe(element, { subtree: true, attributes: true, attributeFilter: ['data-focused'] });
    follow();
    return () => observer.disconnect();
  });

  const diff = () => {
    const snap = s().snapshot;
    if (!snap) return [];
    const rows = SETTINGS.filter((x) => snap[x.id] !== s().recipe[x.id]).map((x) => ({
      name: x.name,
      icon: x.icon,
      a: valueText(x, snap),
      b: valueText(x, s().recipe),
    }));
    if (snap.shiftR !== s().recipe.shiftR || snap.shiftB !== s().recipe.shiftB)
      rows.push({
        name: 'WB SHIFT',
        icon: 'wb',
        a: `R${fmt(snap.shiftR)} B${fmt(snap.shiftB)}`,
        b: `R${fmt(s().recipe.shiftR)} B${fmt(s().recipe.shiftB)}`,
      });
    return rows;
  };

  return (
    <div
      class="focal-review relative h-screen-h w-screen-w overflow-hidden bg-black text-label select-none"
      style={{ '--accent': s().mode === 'raw' ? 'var(--color-play)' : 'var(--color-rec)' }}
    >
      {/* The picture. Split compare: snapshot left of the divider, current right. */}
      <Show
        when={s().compare === 'split' && s().snapshot}
        fallback={<Scene recipe={s().recipe} />}
      >
        {(snap) => (
          <>
            <Scene recipe={s().recipe} />
            <div class="absolute inset-y-0 left-0 overflow-hidden" style={{ width: `${COLUMN + (720 - COLUMN) / 2}px` }}>
              <div class="absolute inset-y-0 left-0 w-screen-w">
                <Scene recipe={snap()} />
              </div>
            </div>
            <div
              class="absolute inset-y-0 w-[3px] -translate-x-1/2 bg-white shadow-[0_0_8px_rgb(0_0_0/0.6)]"
              style={{ left: `${COLUMN + (720 - COLUMN) / 2}px` }}
            />
            <span
              class="absolute bottom-[44px] rounded-[6px] bg-black/60 px-[10px] py-[5px] text-[18px] font-bold"
              style={{ left: `${COLUMN + 14}px` }}
            >
              A
            </span>
            <span class="absolute right-[14px] bottom-[44px] rounded-[6px] bg-(--accent) px-[10px] py-[5px] text-[18px] font-bold">
              B
            </span>
          </>
        )}
      </Show>

      {/* The settings column. */}
      <div
        class="absolute inset-y-0 left-0 flex flex-col bg-[rgb(18_18_20/0.66)] backdrop-blur-[14px]"
        style={{ width: `${COLUMN}px` }}
      >
        <div class="flex shrink-0 items-center justify-between px-[16px]" style={{ height: `${header}px` }}>
          <span class="truncate text-[20px] font-bold tracking-[0.04em] uppercase">{focused().name}</span>
          <Show when={s().snapshot}>
            <span class="ml-[8px] rounded-[4px] border border-white/50 px-[6px] text-[14px] font-bold">A</span>
          </Show>
        </div>
        <div
          ref={(element) => (list = element)}
          onScroll={(e) => setScroll(e.currentTarget.scrollTop)}
          class="relative flex-1 overflow-y-auto [scrollbar-width:none]"
        >
          <For each={SETTINGS}>
            {(setting, i) => (
              <SettingRow
                setting={setting}
                recipe={s().recipe}
                compare={s().snapshot}
                focused={i() === s().focus}
                extended={i() === s().focus && s().fly !== 'none'}
              />
            )}
          </For>
          <div class="h-[40px]" />
        </div>
      </div>

      {/* Top right: what is being edited. */}
      <div class="absolute top-[12px] right-[14px] flex items-center gap-[8px] rounded-[8px] bg-black/55 px-[10px] py-[6px]">
        <Show when={s().mode === 'raw'}>
          <span class="rounded-[4px] bg-(--accent) px-[6px] text-[15px] font-bold">RAW</span>
        </Show>
        <span class="text-[18px] font-bold tracking-[0.03em] uppercase">{s().name}</span>
      </div>

      <Show when={s().fly === 'shift'}>
        <Flyout
          top={rowTop()}
          title="WB SHIFT"
          value={`R ${fmt(s().recipe.shiftR)}  B ${fmt(s().recipe.shiftB)}`}
          height={330}
        >
          <div class="flex flex-1 items-center justify-center">
            <ShiftGrid r={s().recipe.shiftR} b={s().recipe.shiftB} />
          </div>
        </Flyout>
      </Show>
      <Show when={s().fly === 'film'}>
        <Flyout top={rowTop()} title={FILMS[s().recipe.film].long} height={150}>
          <p class="text-[22px] leading-[30px]">{FILMS[s().recipe.film].note}</p>
        </Flyout>
      </Show>

      <Show when={s().compare === 'diff' && s().snapshot}>
        <div
          class="absolute top-[56px] right-[12px] bottom-[48px] flex flex-col rounded-[14px] bg-[rgb(30_30_32/0.88)] p-[16px] backdrop-blur-[18px]"
          style={{ left: `${COLUMN + 12}px` }}
        >
          <div class="mb-[10px] flex items-center text-[16px] font-bold tracking-[0.04em] text-secondary">
            <span class="flex-1" />
            <span class="w-[150px] text-center">A</span>
            <span class="w-[150px] text-center text-label">B</span>
          </div>
          <For each={diff()} fallback={<span class="text-[22px] text-secondary">SAME AS A</span>}>
            {(row) => (
              <div class="flex h-[40px] items-center gap-[10px] shadow-[inset_0_-1px_0_rgb(255_255_255/0.08)]">
                <SettingIcon icon={row.icon as Icon} />
                <span class="flex-1" />
                <span class="w-[150px] truncate text-center text-[17px] text-secondary uppercase">{row.a}</span>
                <span class="w-[150px] truncate text-center text-[17px] font-semibold uppercase">{row.b}</span>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* Key hints along the bottom of the picture. */}
      <div
        class="absolute right-[14px] bottom-[10px] flex items-center justify-end gap-[16px] [text-shadow:0_1px_3px_rgb(0_0_0/0.8)]"
        style={{ left: `${COLUMN + 14}px` }}
      >
        <Show when={focused().id === 'wb'}>
          <Hint keys="▲" label="SHIFT" />
        </Show>
        <Show when={focused().id === 'film'}>
          <Hint keys="DISP" label="INFO" />
        </Show>
        <Show
          when={s().snapshot}
          fallback={<Hint keys="AF-ON" label={s().mode === 'raw' ? 'KEEP AS A' : 'SHOOT A'} />}
        >
          <Hint keys="Q" label={s().compare === 'split' ? 'DIFF' : s().compare === 'diff' ? 'HIDE A' : 'COMPARE'} />
        </Show>
        <Hint keys="OK" label={s().mode === 'raw' ? 'CREATE' : 'SAVE'} />
      </div>
    </div>
  );
}

/* ---- The live screen and gallery --------------------------------------------- */

function step(state: EditorState, delta: number): EditorState {
  const setting = SETTINGS[state.focus];
  const v = state.recipe[setting.id];
  const next =
    setting.kind === 'slider'
      ? Math.max(setting.min, Math.min(setting.max, v + delta * setting.step))
      : Math.max(0, Math.min(setting.choices.length - 1, v + delta));
  return { ...state, recipe: { ...state.recipe, [setting.id]: next } };
}

function onKey(state: EditorState, key: string): EditorState {
  const id = SETTINGS[state.focus].id;
  if (state.fly === 'shift') {
    const r = state.recipe;
    const clamp = (n: number) => Math.max(-9, Math.min(9, n));
    if (key === 'ArrowLeft') return { ...state, recipe: { ...r, shiftR: clamp(r.shiftR - 1) } };
    if (key === 'ArrowRight') return { ...state, recipe: { ...r, shiftR: clamp(r.shiftR + 1) } };
    if (key === 'ArrowUp') return { ...state, recipe: { ...r, shiftB: clamp(r.shiftB + 1) } };
    if (key === 'ArrowDown') return { ...state, recipe: { ...r, shiftB: clamp(r.shiftB - 1) } };
    if (key === 'Enter' || key === 'Escape') return { ...state, fly: 'none' };
    return state;
  }
  if (key === 'ArrowUp' || key === 'ArrowDown') {
    const focus = Math.max(0, Math.min(SETTINGS.length - 1, state.focus + (key === 'ArrowUp' ? -1 : 1)));
    return { ...state, focus, fly: 'none' };
  }
  if (key === 'ArrowLeft') return step(state, -1);
  if (key === 'ArrowRight') return step(state, 1);
  if (key === 's' && id === 'wb') return { ...state, fly: 'shift' };
  if (key === 'i' && id === 'film') return { ...state, fly: state.fly === 'film' ? 'none' : 'film' };
  if (key === 'a') return { ...state, snapshot: { ...state.recipe }, compare: 'off' };
  if (key === 'q' && state.snapshot) {
    const order: CompareView[] = ['split', 'diff', 'off'];
    return { ...state, compare: order[(order.indexOf(state.compare) + 1) % order.length] };
  }
  if (key === 'Escape') return { ...state, fly: 'none', compare: 'off' };
  return state;
}

const INITIAL: EditorState = { mode: 'live', recipe: BASE, focus: 3, fly: 'none', compare: 'off', name: 'C1 KODACHROME' };

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

const PADS = [
  { label: '◀', key: 'ArrowLeft' },
  { label: '▲', key: 'ArrowUp' },
  { label: '▼', key: 'ArrowDown' },
  { label: '▶', key: 'ArrowRight' },
  { label: 'OK', key: 'Enter' },
  { label: 'BACK', key: 'Escape' },
  { label: 'WB SHIFT', key: 's' },
  { label: 'INFO', key: 'i' },
  { label: 'KEEP A', key: 'a' },
  { label: 'COMPARE', key: 'q' },
];

function Live() {
  const [state, setState] = createSignal<EditorState>(INITIAL);
  return (
    <div
      tabindex="0"
      class="outline-none focus-visible:ring-2 focus-visible:ring-[#0068d8]"
      onKeyDown={(e) => {
        const next = onKey(state(), e.key);
        if (next !== state()) {
          e.preventDefault();
          setState(next);
        }
      }}
    >
      <Fit>
        <Editor state={state()} />
      </Fit>
      <div class="mt-[10px] flex max-w-[720px] flex-wrap gap-[8px]">
        <For each={PADS}>
          {(pad) => (
            <button
              type="button"
              class="min-h-[44px] min-w-[52px] rounded-[10px] bg-[#2a2a2c] px-[12px] text-[16px] font-semibold active:bg-[#444]"
              onClick={() => setState(onKey(state(), pad.key))}
            >
              {pad.label}
            </button>
          )}
        </For>
      </div>
    </div>
  );
}

const EDITED: Recipe = { ...BASE, film: 5, shadow: 2, highlight: 0.5, color: -1, grain: 3, shiftR: 4, shiftB: -5 };

const STATES: { id: string; title: string; note?: string; view: () => JSX.Element }[] = [
  { id: 'slider', title: 'Slider row focused', view: () => <Editor state={INITIAL} /> },
  {
    id: 'film',
    title: 'Film simulation carousel',
    note: 'LEFT and RIGHT step through the choices in place.',
    view: () => <Editor state={{ ...INITIAL, focus: 0 }} />,
  },
  {
    id: 'film-info',
    title: 'Film simulation notes fly-out',
    view: () => <Editor state={{ ...INITIAL, focus: 0, fly: 'film' }} />,
  },
  {
    id: 'wb-shift',
    title: 'WB SHIFT fly-out',
    note: 'The row extends into the right column; the selector moves the shift.',
    view: () => <Editor state={{ ...INITIAL, focus: 1, fly: 'shift' }} />,
  },
  {
    id: 'snapshot',
    title: 'Editing after keeping A',
    note: 'Rings mark A on each slider; a dot marks a changed choice.',
    view: () => <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4 }} />,
  },
  {
    id: 'split',
    title: 'Compare: A beside B',
    view: () => <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, compare: 'split' }} />,
  },
  {
    id: 'diff',
    title: 'Compare: what changed',
    view: () => <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, compare: 'diff' }} />,
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    note: 'The same column over the picture being converted.',
    view: () => <Editor state={{ ...INITIAL, mode: 'raw', name: 'DSCF4182', recipe: EDITED, focus: 5 }} />,
  },
];

function Review() {
  return (
    <div class="min-h-dvh bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="text-[22px] font-semibold">Recipe editor</h1>
      <section class="mt-[16px]">
        <h2 class="text-[17px] font-semibold">Live</h2>
        <p class="mt-[4px] mb-[10px] text-[#999]">
          The buttons below the screen, or with a keyboard: arrows, Enter, Esc, S (WB SHIFT), I (INFO),
          A (KEEP A), Q (COMPARE).
        </p>
        <Live />
      </section>
      <For each={STATES}>
        {(state) => (
          <section id={state.id} class="mt-[32px]">
            <h2 class="text-[17px] font-semibold">{state.title}</h2>
            <p class="mt-[4px] mb-[10px] min-h-[1em] text-[#999]">{state.note}</p>
            <Fit>{state.view()}</Fit>
          </section>
        )}
      </For>
    </div>
  );
}

const host = document.getElementById('recipe-editor');
if (!host) throw new Error('Missing #recipe-editor');
if (import.meta.env.DEV) {
  host.replaceChildren();
  render(() => <Review />, host);
}
