import { render } from '@solidjs/web';
import { For, Show, createSignal, onSettled } from 'solid-js';
import { Key } from './components';
import { palette } from './palette';
import type { JSX } from '@solidjs/web';
import './focal.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Fly-outs (WB SHIFT, film simulation notes) extend
   the focused row across the right column. Compare keeps a snapshot recipe
   and shows it beside the current one. The same screen serves RAW CONVERSION
   in playback. */

/* ---- Recipe model ------------------------------------------------------------ */

const FILMS = [
  {
    name: 'PROVIA',
    long: 'PROVIA/STANDARD',
    note: 'Standard reproduction, suitable for a wide range of subjects.',
  },
  {
    name: 'VELVIA',
    long: 'Velvia/VIVID',
    note: 'High contrast with vivid color for landscapes and nature.',
  },
  { name: 'ASTIA', long: 'ASTIA/SOFT', note: 'Soft tones with gentle skin color for portraits.' },
  {
    name: 'CLASSIC CHROME',
    long: 'CLASSIC CHROME',
    note: 'Subdued color and stronger shadow contrast for a documentary look.',
  },
  { name: 'REALA ACE', long: 'REALA ACE', note: 'Faithful color with crisp tonality.' },
  {
    name: 'CLASSIC Neg.',
    long: 'CLASSIC Neg.',
    note: 'Hard tonality with distinct color, like a snapshot on negative film.',
  },
  { name: 'ETERNA', long: 'ETERNA/CINEMA', note: 'Muted color and rich shadows, for movies.' },
  { name: 'ACROS', long: 'ACROS', note: 'Monochrome with smooth gradation and fine sharpness.' },
] as const;

const WB = ['AUTO', 'AUTO WHITE', 'DAYLIGHT', 'SHADE', 'FLUOR. 1', 'INCAND.', '5600K'] as const;
const EFFECT = ['OFF', 'WEAK', 'STRONG'] as const;

type Icon =
  | 'film'
  | 'wb'
  | 'grain'
  | 'chrome'
  | 'fxblue'
  | 'highlight'
  | 'shadow'
  | 'color'
  | 'sharp'
  | 'nr'
  | 'clarity'
  | 'dr';

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
  {
    kind: 'choice',
    id: 'film',
    name: 'FILM SIMULATION',
    icon: 'film',
    choices: FILMS.map((f) => f.name),
  },
  { kind: 'choice', id: 'wb', name: 'WHITE BALANCE', icon: 'wb', choices: WB },
  {
    kind: 'choice',
    id: 'dr',
    name: 'DYNAMIC RANGE',
    icon: 'dr',
    choices: ['AUTO', 'DR100', 'DR200', 'DR400'],
  },
  {
    kind: 'slider',
    id: 'highlight',
    name: 'HIGHLIGHT',
    icon: 'highlight',
    min: -2,
    max: 4,
    step: 0.5,
  },
  { kind: 'slider', id: 'shadow', name: 'SHADOW', icon: 'shadow', min: -2, max: 4, step: 0.5 },
  { kind: 'slider', id: 'color', name: 'COLOR', icon: 'color', min: -4, max: 4, step: 1 },
  { kind: 'choice', id: 'chrome', name: 'COLOR CHROME EFFECT', icon: 'chrome', choices: EFFECT },
  { kind: 'choice', id: 'fxblue', name: 'COLOR CHROME FX BLUE', icon: 'fxblue', choices: EFFECT },
  {
    kind: 'choice',
    id: 'grain',
    name: 'GRAIN EFFECT',
    icon: 'grain',
    choices: ['OFF', 'WEAK SMALL', 'WEAK LARGE', 'STRONG SMALL', 'STRONG LARGE'],
  },
  { kind: 'slider', id: 'clarity', name: 'CLARITY', icon: 'clarity', min: -5, max: 5, step: 1 },
  { kind: 'slider', id: 'sharp', name: 'SHARPNESS', icon: 'sharp', min: -4, max: 4, step: 1 },
  { kind: 'slider', id: 'nr', name: 'HIGH ISO NR', icon: 'nr', min: -4, max: 4, step: 1 },
];

/** Slider settings hold a number, choice settings the index of their choice. */
type Recipe = Record<string, number> & { shiftR: number; shiftB: number };

const BASE: Recipe = {
  film: 3,
  wb: 0,
  dr: 0,
  highlight: -1,
  shadow: 1,
  color: 2,
  chrome: 1,
  fxblue: 0,
  grain: 1,
  clarity: 0,
  sharp: -1,
  nr: -4,
  shiftR: 2,
  shiftB: -3,
};

const fmt = (n: number) => (n > 0 ? `+${n}` : `${n}`);

function valueText(setting: Setting, recipe: Recipe) {
  const v = recipe[setting.id];
  return setting.kind === 'slider' ? fmt(v) : setting.choices[v];
}

/* ---- Icons ------------------------------------------------------------------- */

function SettingIcon(props: { icon: Icon }) {
  const s = {
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': 2.4,
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
  } as const;
  const shapes: Record<Icon, JSX.Element> = {
    film: (
      <>
        <rect x="5" y="4" width="22" height="24" rx="2" {...s} />
        <path
          d="M10 4v24M22 4v24M5 10h5M5 16h5M5 22h5M22 10h5M22 16h5M22 22h5"
          {...s}
          stroke-width="1.8"
        />
      </>
    ),
    wb: (
      <>
        <text
          x="16"
          y="21"
          text-anchor="middle"
          font-size="14"
          font-weight="800"
          fill="currentColor"
        >
          WB
        </text>
        <rect x="3" y="6" width="26" height="20" rx="3" {...s} />
      </>
    ),
    dr: (
      <>
        <text
          x="16"
          y="21"
          text-anchor="middle"
          font-size="13"
          font-weight="800"
          fill="currentColor"
        >
          DR
        </text>
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
        <text
          x="16"
          y="23"
          text-anchor="middle"
          font-size="9"
          font-weight="800"
          fill="currentColor"
        >
          B
        </text>
      </>
    ),
    grain: (
      <>
        <rect x="4" y="4" width="24" height="24" rx="3" {...s} />
        <For
          each={[
            [10, 10],
            [17, 9],
            [23, 12],
            [12, 17],
            [20, 18],
            [9, 23],
            [16, 23],
            [24, 24],
          ]}
        >
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
        <text
          x="16"
          y="21"
          text-anchor="middle"
          font-size="12"
          font-weight="800"
          fill="currentColor"
        >
          NR
        </text>
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

/* ---- Live view --------------------------------------------------------------- */

/** An approximation of the recipe's look as CSS filters, enough to see edits move. */
function look(r: Recipe): string {
  const film = FILMS[r.film].name;
  const mono = film === 'ACROS';
  const sat = {
    PROVIA: 1,
    VELVIA: 1.45,
    ASTIA: 0.95,
    'CLASSIC CHROME': 0.72,
    'REALA ACE': 1.02,
    'CLASSIC Neg.': 0.85,
    ETERNA: 0.6,
    ACROS: 0,
  }[film];
  const contrast =
    1 + (r.shadow + r.highlight) * 0.06 + (film === 'CLASSIC Neg.' ? 0.12 : 0) + r.clarity * 0.02;
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
    <div
      class={`absolute inset-0 overflow-hidden ${props.class ?? ''}`}
      style={{ filter: look(props.recipe) }}
    >
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
              <rect
                x={162}
                y={110 + i * 42}
                width="22"
                height="26"
                fill="#f2d48a"
                opacity={i % 2 ? 0.9 : 0.35}
              />
              <rect
                x={204}
                y={110 + i * 42}
                width="22"
                height="26"
                fill="#f2d48a"
                opacity={i % 3 ? 0.4 : 0.95}
              />
              <rect
                x={616}
                y={140 + i * 38}
                width="18"
                height="22"
                fill="#e9b765"
                opacity={i % 2 ? 0.3 : 0.85}
              />
              <rect
                x={660}
                y={140 + i * 38}
                width="18"
                height="22"
                fill="#e9b765"
                opacity={i % 2 ? 0.85 : 0.3}
              />
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

/* ---- Layout ------------------------------------------------------------------ */

const TITLE = 48;
const ROW = 58;
const FOOTER = 40;
const LIST = 480 - TITLE - FOOTER;
const WIDTH = 330;
const COMPARE_WIDTH = 300;
const CELL = 50;

/** The same scroll for every column, so A and B stay in lockstep. */
function scrollFor(focus: number): number {
  const max = Math.max(0, SETTINGS.length * ROW - LIST);
  return Math.max(0, Math.min(max, focus * ROW - (LIST - ROW) / 2));
}

/** Flat translucent panels, as the stock live pickers draw them. */
const PANEL = 'rgb(25 25 25 / 0.82)';
const CARD = 'rgb(60 60 60 / 0.85)';
const LIGHT = 'font-light tracking-[0.08em] uppercase';

/* ---- Choice glyphs ----------------------------------------------------------- */

const GLYPHS: Record<string, string[]> = {
  film: ['STD', 'V', 'S', 'Cc', 'R', 'Nc', 'E', 'A'],
  wb: ['AUTO', 'AW', 'sun', 'shade', 'F1', 'bulb', 'K'],
  dr: ['AUTO', '100', '200', '400'],
  chrome: ['OFF', 'W', 'S'],
  fxblue: ['OFF', 'W', 'S'],
  grain: ['OFF', 'WS', 'WL', 'SS', 'SL'],
};

function Glyph(props: { setting: string; index: number }) {
  const glyph = () => GLYPHS[props.setting][props.index];
  const line = {
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': 2,
    'stroke-linecap': 'round',
  } as const;
  return (
    <svg width="38" height="34" viewBox="0 0 38 34">
      <Show
        when={props.setting === 'film'}
        fallback={
          <Show
            when={['sun', 'shade', 'bulb'].includes(glyph())}
            fallback={
              <>
                <rect x="2" y="5" width="34" height="24" rx="3" {...line} />
                <text
                  x="19"
                  y="22"
                  text-anchor="middle"
                  font-size={glyph().length > 2 ? '11' : '14'}
                  font-weight="700"
                  fill="currentColor"
                >
                  {glyph()}
                </text>
              </>
            }
          >
            <>
              <Show when={glyph() === 'sun'}>
                <circle cx="19" cy="17" r="6" fill="currentColor" />
                <path
                  d="M19 3v4M19 27v4M5 17h4M29 17h4M9 7l3 3M26 24l3 3M29 7l-3 3M12 24l-3 3"
                  {...line}
                />
              </Show>
              <Show when={glyph() === 'shade'}>
                <path d="M6 16l13-10 13 10M9 14v14h20V14" {...line} />
                <path d="M13 28v-8h12v8" fill="currentColor" />
              </Show>
              <Show when={glyph() === 'bulb'}>
                <path d="M13 18a7 7 0 1 1 12 0c-2 2-2 4-2 6h-8c0-2 0-4-2-6zM15 28h8" {...line} />
              </Show>
            </>
          </Show>
        }
      >
        {/* A film canister: the body, and the spool post on top. */}
        <path d="M4 7h26v4h4v20H4z" fill="currentColor" />
        <rect x="24" y="3" width="6" height="4" fill="currentColor" />
        <text
          x="17"
          y="26"
          text-anchor="middle"
          font-size={glyph().length > 2 ? '11' : '15'}
          font-weight="800"
          class="fill-[var(--glyph-legend)]"
        >
          {glyph()}
        </text>
      </Show>
    </svg>
  );
}

/** Every choice in a row as an icon, scrolled to keep the chosen one in view. */
function IconStrip(props: {
  setting: ChoiceSetting;
  value: number;
  width: number;
  focused: boolean;
}) {
  const count = () => props.setting.choices.length;
  const offset = () => {
    const max = Math.max(0, count() * CELL - props.width);
    return Math.max(0, Math.min(max, props.value * CELL - (props.width - CELL) / 2));
  };
  return (
    <div class="relative h-[46px] overflow-hidden" style={{ width: `${props.width}px` }}>
      <div
        class="absolute inset-y-0 left-0 flex"
        style={{ transform: `translateX(${-offset()}px)` }}
      >
        <For each={props.setting.choices.map((_, i) => i)}>
          {(i) => {
            const chosen = () => i === props.value;
            return (
              <div
                class="flex h-[46px] items-center justify-center"
                style={{
                  width: `${CELL}px`,
                  background: chosen()
                    ? props.focused
                      ? '#ffffff'
                      : 'rgb(255 255 255 / 0.22)'
                    : undefined,
                  color:
                    chosen() && props.focused
                      ? '#000000'
                      : chosen()
                        ? '#ffffff'
                        : 'rgb(255 255 255 / 0.5)',
                  '--glyph-legend': chosen() && props.focused ? '#ffffff' : '#1a1a1a',
                }}
              >
                <Glyph setting={props.setting.id} index={i} />
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

/* ---- Rows -------------------------------------------------------------------- */

function Ticks(props: { setting: SliderSetting; value: number; focused: boolean }) {
  const steps = () => Math.round((props.setting.max - props.setting.min) / props.setting.step) + 1;
  const at = (v: number) =>
    ((v - props.setting.min) / (props.setting.max - props.setting.min)) * 100;
  return (
    <div class="relative h-[30px] flex-1">
      <For
        each={Array.from({ length: steps() }, (_, i) => props.setting.min + i * props.setting.step)}
      >
        {(v) => (
          <div
            class="absolute bottom-[4px] w-[2px] -translate-x-1/2"
            style={{
              left: `${at(v)}%`,
              height: v === 0 ? '18px' : Number.isInteger(v) ? '10px' : '5px',
              background: v === 0 ? '#ffffff' : 'rgb(255 255 255 / 0.45)',
            }}
          />
        )}
      </For>
      <div class="absolute inset-x-0 bottom-[3px] h-[1px] bg-white/45" />
      <div
        class="absolute bottom-0 h-[26px] w-[6px] -translate-x-1/2 bg-(--accent)"
        style={{ left: `${at(props.value)}%` }}
      />
    </div>
  );
}

function SettingRow(props: {
  setting: Setting;
  recipe: Recipe;
  focused: boolean;
  changed: boolean;
  width: number;
}) {
  return (
    <div
      class={`relative flex items-center gap-[12px] pr-[14px] pl-[16px] ${props.focused ? 'focal-focused' : 'focal-bevel'}`}
      style={{ height: `${ROW}px` }}
    >
      <SettingIcon icon={props.setting.icon} />
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <IconStrip
            setting={props.setting as ChoiceSetting}
            value={props.recipe[props.setting.id]}
            width={props.width - 72}
            focused={props.focused}
          />
        }
      >
        {(slider) => (
          <>
            <Ticks setting={slider()} value={props.recipe[slider().id]} focused={props.focused} />
            <span
              class="w-[64px] text-right text-[26px] leading-none tabular-nums"
              style={{ color: props.changed ? 'var(--accent-text)' : undefined }}
            >
              {fmt(props.recipe[slider().id])}
            </span>
          </>
        )}
      </Show>
      <Show when={props.changed && props.setting.kind === 'choice'}>
        <span class="absolute top-[6px] right-[6px] h-[8px] w-[8px] bg-(--accent)" />
      </Show>
    </div>
  );
}

/** The settings column. A compare column renders the same rows for A or B. */
function Column(props: {
  recipe: Recipe;
  other?: Recipe;
  focus: number;
  left: number;
  width: number;
  badge?: string;
}) {
  const focused = () => SETTINGS[props.focus];
  return (
    <div
      class="absolute top-0 flex flex-col"
      style={{
        left: `${props.left}px`,
        width: `${props.width}px`,
        height: `${480 - FOOTER}px`,
        background: PANEL,
      }}
    >
      <div class="flex shrink-0 items-center gap-[10px] px-[16px]" style={{ height: `${TITLE}px` }}>
        <Show when={props.badge}>
          <span class="flex h-[28px] w-[28px] items-center justify-center bg-white text-[18px] font-bold text-black">
            {props.badge}
          </span>
        </Show>
        <span class={`truncate text-[22px] ${LIGHT}`}>{focused().name}</span>
      </div>
      <div class="relative flex-1 overflow-hidden">
        <div style={{ transform: `translateY(${-scrollFor(props.focus)}px)` }}>
          <For each={SETTINGS}>
            {(setting, i) => (
              <SettingRow
                setting={setting}
                recipe={props.recipe}
                focused={i() === props.focus}
                changed={
                  props.other !== undefined && props.other[setting.id] !== props.recipe[setting.id]
                }
                width={props.width}
              />
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

/* ---- Fly-outs ---------------------------------------------------------------- */

/** A stock-style title card level with the row, and a body card beneath it. */
function Flyout(props: {
  row: number;
  left: number;
  title: string;
  value?: string;
  children: JSX.Element;
}) {
  return (
    <div
      class="absolute flex flex-col gap-[4px]"
      style={{ left: `${props.left}px`, right: '12px', top: `${props.row}px` }}
    >
      <div
        class={`flex items-center justify-between px-[14px] text-[26px] ${LIGHT}`}
        style={{ height: `${ROW}px`, background: CARD }}
      >
        <span>{props.title}</span>
        <span class="tabular-nums">{props.value}</span>
      </div>
      <div class="px-[14px] py-[12px]" style={{ background: CARD }}>
        {props.children}
      </div>
    </div>
  );
}

/** WB SHIFT: R on the horizontal axis, B on the vertical, -9 to +9 as stock. */
function ShiftGrid(props: { r: number; b: number }) {
  const cell = 11;
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
                <line
                  x1={i * cell}
                  y1="0"
                  x2={i * cell}
                  y2={size}
                  stroke="rgb(255 255 255 / 0.12)"
                />
                <line
                  x1="0"
                  y1={i * cell}
                  x2={size}
                  y2={i * cell}
                  stroke="rgb(255 255 255 / 0.12)"
                />
              </>
            )}
          </For>
          <line x1={size / 2} y1="0" x2={size / 2} y2={size} stroke="rgb(255 255 255 / 0.5)" />
          <line x1="0" y1={size / 2} x2={size} y2={size / 2} stroke="rgb(255 255 255 / 0.5)" />
        </svg>
        <div
          class="absolute rounded-[3px] border-[3px] border-white bg-(--accent) shadow-[0_1px_6px_rgb(0_0_0/0.7)]"
          style={{
            left: `${pos(props.r) - 3}px`,
            top: `${pos(-props.b) - 3}px`,
            width: `${cell + 6}px`,
            height: `${cell + 6}px`,
          }}
        />
        <span class="absolute -top-[2px] left-1/2 -translate-x-1/2 -translate-y-full text-[16px] font-bold">
          B
        </span>
        <span class="absolute top-1/2 -right-[4px] translate-x-full -translate-y-1/2 text-[16px] font-bold">
          R
        </span>
        <span class="absolute -bottom-[2px] left-1/2 -translate-x-1/2 translate-y-full text-[16px] font-bold">
          Y
        </span>
        <span class="absolute top-1/2 -left-[4px] -translate-x-full -translate-y-1/2 text-[16px] font-bold">
          G
        </span>
      </div>
    </div>
  );
}

/* ---- The editor screen ------------------------------------------------------- */

type Fly = 'none' | 'shift' | 'film';
type Mode = 'live' | 'raw';

interface EditorState {
  mode: Mode;
  recipe: Recipe;
  focus: number;
  fly: Fly;
  /** The recipe kept as A. */
  snapshot?: Recipe;
  comparing: boolean;
  name: string;
}

function Hint(props: { keys: string; label: string }) {
  return (
    <span class="flex items-center gap-[6px]">
      <Key>{props.keys}</Key>
      <span class={`text-[18px] ${LIGHT}`}>{props.label}</span>
    </span>
  );
}

function Editor(props: { state: EditorState }) {
  const s = () => props.state;
  const focused = () => SETTINGS[s().focus];
  const rowTop = () => TITLE + s().focus * ROW - scrollFor(s().focus);
  const colors = palette(null);
  const choiceName = () => {
    const f = focused();
    if (f.kind !== 'choice') return undefined;
    return f.id === 'film' ? FILMS[s().recipe.film].long : f.choices[s().recipe[f.id]];
  };
  const split = 360;

  return (
    <div
      class="focal-review relative h-screen-h w-screen-w overflow-hidden bg-black text-label select-none"
      style={{
        '--hairline': colors.hairline,
        '--focus': colors.focus,
        '--accent': s().mode === 'raw' ? 'var(--color-play)' : 'var(--color-setup)',
        '--accent-text': s().mode === 'raw' ? '#3ad06a' : '#4aa3ff',
      }}
    >
      <Scene recipe={s().recipe} />
      <Show when={s().comparing && s().snapshot}>
        {(a) => (
          <>
            <div class="absolute inset-y-0 left-0 overflow-hidden" style={{ width: `${split}px` }}>
              <div class="absolute inset-y-0 left-0 w-screen-w">
                <Scene recipe={a()} />
              </div>
            </div>
            <div class="absolute inset-y-0 w-[2px] bg-white" style={{ left: `${split - 1}px` }} />
            <Column recipe={a()} focus={s().focus} left={0} width={COMPARE_WIDTH} badge="A" />
            <Column
              recipe={s().recipe}
              other={a()}
              focus={s().focus}
              left={720 - COMPARE_WIDTH}
              width={COMPARE_WIDTH}
              badge="B"
            />
          </>
        )}
      </Show>

      <Show when={!s().comparing || !s().snapshot}>
        <Column recipe={s().recipe} other={s().snapshot} focus={s().focus} left={0} width={WIDTH} />
        <Show when={s().fly === 'none'}>
          <div
            class="absolute top-[10px] right-[12px] flex items-center gap-[8px] px-[14px] py-[6px]"
            style={{ background: CARD }}
          >
            <Show when={s().mode === 'raw' && !choiceName()}>
              <span class="bg-(--accent) px-[6px] text-[16px] font-bold">RAW</span>
            </Show>
            <span class={`text-[24px] ${LIGHT}`}>{choiceName() ?? s().name}</span>
          </div>
        </Show>
        <Show when={s().fly === 'film'}>
          <Flyout row={rowTop()} left={WIDTH} title={FILMS[s().recipe.film].long}>
            <p class="text-[22px] leading-[30px] font-light tracking-[0.04em]">
              {FILMS[s().recipe.film].note}
            </p>
          </Flyout>
        </Show>
        <Show when={s().fly === 'shift'}>
          <Flyout
            row={Math.min(rowTop(), 480 - FOOTER - ROW - 4 - 290 - 8)}
            left={WIDTH}
            title="WB SHIFT"
            value={`R${fmt(s().recipe.shiftR)} B${fmt(s().recipe.shiftB)}`}
          >
            <div class="flex justify-center py-[22px]">
              <ShiftGrid r={s().recipe.shiftR} b={s().recipe.shiftB} />
            </div>
          </Flyout>
        </Show>
      </Show>

      <div
        class="absolute inset-x-0 bottom-0 flex items-center justify-end gap-[18px] px-[14px]"
        style={{ height: `${FOOTER}px`, background: 'rgb(0 0 0 / 0.72)' }}
      >
        <Show when={focused().id === 'wb' && s().fly === 'none'}>
          <Hint keys="▲" label="SHIFT" />
        </Show>
        <Show when={focused().id === 'film' && s().fly === 'none'}>
          <Hint keys="DISP" label="INFO" />
        </Show>
        <Show
          when={s().snapshot}
          fallback={<Hint keys="AF-ON" label={s().mode === 'raw' ? 'KEEP A' : 'SHOOT A'} />}
        >
          <Hint keys="Q" label={s().comparing ? 'HIDE A' : 'COMPARE'} />
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
    const focus = Math.max(
      0,
      Math.min(SETTINGS.length - 1, state.focus + (key === 'ArrowUp' ? -1 : 1)),
    );
    return { ...state, focus, fly: 'none' };
  }
  if (key === 'ArrowLeft') return step(state, -1);
  if (key === 'ArrowRight') return step(state, 1);
  if (key === 's' && id === 'wb' && !state.comparing) return { ...state, fly: 'shift' };
  if (key === 'i' && id === 'film' && !state.comparing)
    return { ...state, fly: state.fly === 'film' ? 'none' : 'film' };
  if (key === 'a') return { ...state, snapshot: { ...state.recipe }, comparing: false };
  if (key === 'q' && state.snapshot) return { ...state, comparing: !state.comparing, fly: 'none' };
  if (key === 'Escape') return { ...state, fly: 'none', comparing: false };
  return state;
}

const INITIAL: EditorState = {
  mode: 'live',
  recipe: BASE,
  focus: 3,
  fly: 'none',
  comparing: false,
  name: 'C1 KODACHROME',
};

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
    <div
      ref={(element) => (host = element)}
      class="w-full max-w-[720px]"
      style={{ height: `${480 * scale()}px` }}
    >
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

const EDITED: Recipe = {
  ...BASE,
  film: 5,
  shadow: 2,
  highlight: 0.5,
  color: -1,
  grain: 3,
  shiftR: 4,
  shiftB: -5,
};

const STATES: { id: string; title: string; view: () => JSX.Element }[] = [
  { id: 'slider', title: 'Slider row', view: () => <Editor state={INITIAL} /> },
  {
    id: 'film',
    title: 'Film simulation icons',
    view: () => <Editor state={{ ...INITIAL, focus: 0 }} />,
  },
  {
    id: 'film-info',
    title: 'Film simulation notes',
    view: () => <Editor state={{ ...INITIAL, focus: 0, fly: 'film' }} />,
  },
  {
    id: 'wb',
    title: 'White balance icons',
    view: () => <Editor state={{ ...INITIAL, focus: 1, recipe: { ...BASE, wb: 3 } }} />,
  },
  {
    id: 'wb-shift',
    title: 'WB SHIFT',
    view: () => <Editor state={{ ...INITIAL, focus: 1, fly: 'shift' }} />,
  },
  {
    id: 'compare',
    title: 'Compare A and B',
    view: () => (
      <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true }} />
    ),
  },
  {
    id: 'compare-film',
    title: 'Compare A and B, film simulation',
    view: () => (
      <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 0, comparing: true }} />
    ),
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    view: () => (
      <Editor state={{ ...INITIAL, mode: 'raw', name: 'DSCF4182', recipe: EDITED, focus: 5 }} />
    ),
  },
];

function Review() {
  return (
    <div class="min-h-dvh bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="text-[22px] font-semibold">Recipe editor</h1>
      <section class="mt-[16px]">
        <Live />
      </section>
      <For each={STATES}>
        {(state) => (
          <section id={state.id} class="mt-[32px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{state.title}</h2>
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
