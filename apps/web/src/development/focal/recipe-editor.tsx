import { render } from '@solidjs/web';
import { For, Show, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { Key } from './components';
import { palette } from './palette';
import './focal.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Choice rows scroll their choices as stock icons;
   fly-outs (WB SHIFT, film notes) extend the focused row across the picture.
   Compare shows the column twice, A and B, scrolled together. The same screen
   serves RAW CONVERSION in playback.

   Stock icons come from `mise run stock-icons-svg -- --firmware x70-110`,
   served from the local review directory; they are never published. */

const STOCK = '/review/stock-icons/x70-110';
const MOD = '/review/recipe-editor/mod-icons';
const stock = (id: number) => `${STOCK}/ff${id.toString(16).padStart(6, '0')}.svg`;

/* ---- Recipe model ------------------------------------------------------------ */

interface Choice {
  name: string;
  /** Stock vector id, or none for a text choice. */
  icon?: number;
  note?: string;
}

const FILMS: Choice[] = [
  {
    name: 'PROVIA/STANDARD',
    icon: 0x1dc,
    note: 'Standard color reproduction for a wide range of subjects.',
  },
  {
    name: 'Velvia/VIVID',
    icon: 0x1dd,
    note: 'High-contrast reproduction with vivid colors, for landscapes.',
  },
  { name: 'ASTIA/SOFT', icon: 0x1de, note: 'Soft tones and gentle color, for portraits.' },
  {
    name: 'CLASSIC CHROME',
    icon: 0x1e5,
    note: 'Soft color and enhanced shadow contrast for a calm look.',
  },
  {
    name: 'PRO Neg. Hi',
    icon: 0x1df,
    note: 'Slightly higher contrast than PRO Neg. Std, for portraits outdoors.',
  },
  {
    name: 'PRO Neg. Std',
    icon: 0x1e0,
    note: 'Soft gradation and skin tones, for studio portraits.',
  },
  { name: 'MONOCHROME', icon: 0x1e1, note: 'Standard black and white.' },
  {
    name: 'MONOCHROME+Ye FILTER',
    icon: 0x1e2,
    note: 'Black and white with slightly higher contrast.',
  },
  {
    name: 'MONOCHROME+R FILTER',
    icon: 0x1e3,
    note: 'Black and white with higher contrast and darker skies.',
  },
  { name: 'MONOCHROME+G FILTER', icon: 0x1e4, note: 'Black and white with softer skin tones.' },
  { name: 'SEPIA', icon: 0x1e6, note: 'Sepia tone.' },
];

const WB: Choice[] = [
  { name: 'AUTO', icon: 0x1d2 },
  { name: 'CUSTOM 1', icon: 0x1d3 },
  { name: 'COLOR TEMPERATURE', icon: 0x1d4 },
  { name: 'FINE', icon: 0x1d5 },
  { name: 'SHADE', icon: 0x1d6 },
  { name: 'FLUORESCENT LIGHT-1', icon: 0x1d7 },
  { name: 'FLUORESCENT LIGHT-2', icon: 0x1d8 },
  { name: 'FLUORESCENT LIGHT-3', icon: 0x1d9 },
  { name: 'INCANDESCENT LIGHT', icon: 0x1da },
  { name: 'UNDERWATER', icon: 0x1db },
];

const DR: Choice[] = [
  { name: 'AUTO', icon: 0x575 },
  { name: 'DR100', icon: 0x572 },
  { name: 'DR200', icon: 0x573 },
  { name: 'DR400', icon: 0x574 },
];

const EFFECT: Choice[] = [{ name: 'OFF' }, { name: 'WEAK' }, { name: 'STRONG' }];
const GRAIN: Choice[] = [{ name: 'OFF' }, { name: 'WEAK' }, { name: 'STRONG' }];

/** A row's icon: a stock Q menu vector, or a mod's own artwork. */
type RowIcon = { stock: number } | { mod: string };

interface SliderSetting {
  kind: 'slider';
  id: string;
  name: string;
  icon: RowIcon;
  min: number;
  max: number;
}
interface ChoiceSetting {
  kind: 'choice';
  id: string;
  name: string;
  icon: RowIcon;
  choices: Choice[];
}
type Setting = SliderSetting | ChoiceSetting;

const SETTINGS: Setting[] = [
  { kind: 'choice', id: 'film', name: 'FILM SIMULATION', icon: { stock: 0x5f3 }, choices: FILMS },
  { kind: 'choice', id: 'wb', name: 'WHITE BALANCE', icon: { stock: 0x609 }, choices: WB },
  { kind: 'choice', id: 'dr', name: 'DYNAMIC RANGE', icon: { stock: 0x5f1 }, choices: DR },
  {
    kind: 'slider',
    id: 'highlight',
    name: 'HIGHLIGHT TONE',
    icon: { stock: 0x5f6 },
    min: -2,
    max: 2,
  },
  { kind: 'slider', id: 'shadow', name: 'SHADOW TONE', icon: { stock: 0x607 }, min: -2, max: 2 },
  { kind: 'slider', id: 'color', name: 'COLOR', icon: { stock: 0x5f0 }, min: -2, max: 2 },
  { kind: 'slider', id: 'sharp', name: 'SHARPNESS', icon: { stock: 0x605 }, min: -2, max: 2 },
  { kind: 'slider', id: 'nr', name: 'NOISE REDUCTION', icon: { stock: 0x603 }, min: -2, max: 2 },
  {
    kind: 'choice',
    id: 'chrome',
    name: 'COLOR CHROME EFFECT',
    icon: { mod: 'color_chrome' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'fxblue',
    name: 'COLOR CHROME FX BLUE',
    icon: { mod: 'color_chrome_blue' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'grain',
    name: 'GRAIN EFFECT',
    icon: { mod: 'film_grain' },
    choices: GRAIN,
  },
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
  sharp: -1,
  nr: -2,
  chrome: 1,
  fxblue: 0,
  grain: 1,
  shiftR: 2,
  shiftB: -3,
};

const fmt = (n: number) => (n > 0 ? `+${n}` : `${n}`);

/** An approximation of the recipe's look as CSS filters, enough to see edits move. */
function look(r: Recipe): string {
  const film = r.film;
  const mono = film >= 6 && film <= 9;
  const sepia = film === 10;
  const sat = [1, 1.45, 0.95, 0.72, 1.05, 0.9][film] ?? 0;
  const contrast = 1 + (r.shadow + r.highlight) * 0.07 + (film === 4 ? 0.08 : 0);
  const warm = r.shiftR * 2 - r.shiftB * 2 + ([0, 0, 4, 0, 10, -14, -14, -14, -24, 2][r.wb] ?? 0);
  return [
    `saturate(${mono || sepia ? 0 : sat * (1 + r.color * 0.15) * (1 + r.chrome * 0.04)})`,
    `contrast(${contrast + (film === 8 ? 0.15 : 0)})`,
    `brightness(${1 - r.shadow * 0.03 + 0.02})`,
    `sepia(${sepia ? 0.8 : Math.max(0, warm) / 60})`,
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

/* ---- Layout ------------------------------------------------------------------ */

const ROW = 62;
const WIDTH = 340;
const COMPARE_WIDTH = 300;
const COMPARE_HEADER = 32;
const ICON_CELL = 52;
const TEXT_CELL = 92;

/** The same scroll for every column, so A and B stay in lockstep. */
function scrollFor(focus: number, height: number): number {
  const max = Math.max(0, SETTINGS.length * ROW - height);
  return Math.max(0, Math.min(max, focus * ROW - (height - ROW) / 2));
}

/** Flat translucent panels, as the stock live pickers draw them. */
const PANEL = 'rgb(25 25 25 / 0.82)';
const CARD = 'rgb(60 60 60 / 0.85)';
const LIGHT = 'font-light tracking-[0.08em] uppercase';

function RowIconView(props: { icon: RowIcon }) {
  return (
    <img
      alt=""
      class="h-[28px] w-[46px] shrink-0 object-contain"
      style={{ filter: 'invert(1)' }}
      src={'stock' in props.icon ? stock(props.icon.stock) : `${MOD}/${props.icon.mod}.svg`}
    />
  );
}

/* ---- Rows -------------------------------------------------------------------- */

/** Every choice in a row, scrolled to keep the chosen one in view. */
function ChoiceStrip(props: {
  setting: ChoiceSetting;
  value: number;
  width: number;
  focused: boolean;
}) {
  const cell = () => (props.setting.choices[0].icon ? ICON_CELL : TEXT_CELL);
  const offset = () => {
    const max = Math.max(0, props.setting.choices.length * cell() - props.width);
    return Math.max(0, Math.min(max, props.value * cell() - (props.width - cell()) / 2));
  };
  return (
    <div class="relative h-[34px] overflow-hidden" style={{ width: `${props.width}px` }}>
      <div
        class="absolute inset-y-0 left-0 flex"
        style={{ transform: `translateX(${-offset()}px)` }}
      >
        <For each={props.setting.choices.map((choice, i) => ({ choice, i }))}>
          {(item) => {
            const chosen = () => item.i === props.value;
            const inverted = () => chosen() && props.focused;
            return (
              <div
                class="flex h-[34px] shrink-0 items-center justify-center"
                style={{
                  width: `${cell()}px`,
                  background: inverted()
                    ? '#ffffff'
                    : chosen()
                      ? 'rgb(255 255 255 / 0.2)'
                      : undefined,
                  opacity: chosen() ? 1 : 0.5,
                }}
              >
                <Show
                  when={item.choice.icon}
                  fallback={
                    <span
                      class={`text-[18px] ${LIGHT}`}
                      
                    >
                      {item.choice.name}
                    </span>
                  }
                >
                  {(icon) => (
                    <img
                      alt=""
                      class="h-[26px] w-[46px] object-contain"
                                            src={stock(icon())}
                    />
                  )}
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

function Ticks(props: { setting: SliderSetting; value: number }) {
  const at = (v: number) =>
    ((v - props.setting.min) / (props.setting.max - props.setting.min)) * 100;
  return (
    <div class="relative mx-[8px] h-[30px] flex-1">
      <For
        each={Array.from(
          { length: props.setting.max - props.setting.min + 1 },
          (_, i) => props.setting.min + i,
        )}
      >
        {(v) => (
          <div
            class="absolute bottom-[4px] w-[2px] -translate-x-1/2"
            style={{
              left: `${at(v)}%`,
              height: v === 0 ? '18px' : '10px',
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
  const controlWidth = () => props.width - 16 - 46 - 12 - 12;
  const choiceName = () =>
    props.setting.kind === 'choice'
      ? props.setting.choices[props.recipe[props.setting.id]].name
      : '';
  return (
    <div
      class={`relative flex items-center gap-[12px] pr-[12px] pl-[16px] ${props.focused ? 'focal-focused' : 'focal-bevel'}`}
      style={{ height: `${ROW}px` }}
    >
      <RowIconView icon={props.setting.icon} />
      <div class="flex min-w-0 flex-1 flex-col">
        <div
          class={`flex h-[18px] items-center justify-between text-[14px] text-secondary ${LIGHT}`}
        >
          <span class="truncate">{props.setting.name}</span>
          <Show when={props.setting.kind === 'choice' && props.setting.choices[0].icon}>
            <span class="ml-[8px] truncate text-label">{choiceName()}</span>
          </Show>
        </div>
        <Show
          when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
          fallback={
            <ChoiceStrip
              setting={props.setting as ChoiceSetting}
              value={props.recipe[props.setting.id]}
              width={controlWidth()}
              focused={props.focused}
            />
          }
        >
          {(slider) => (
            <div class="flex h-[34px] items-center">
              <Ticks setting={slider()} value={props.recipe[slider().id]} />
              <span
                class="w-[44px] text-right text-[24px] leading-none tabular-nums"
                style={{ color: props.changed ? 'var(--accent-text)' : undefined }}
              >
                {fmt(props.recipe[slider().id])}
              </span>
            </div>
          )}
        </Show>
      </div>
      <Show when={props.changed && props.setting.kind === 'choice'}>
        <span class="absolute top-[6px] right-[6px] h-[8px] w-[8px] bg-(--accent)" />
      </Show>
    </div>
  );
}

/** The settings column. In compare, the same rows render once for A and once for B. */
function Column(props: {
  recipe: Recipe;
  other?: Recipe;
  focus: number;
  left: number;
  width: number;
  badge?: string;
  active?: boolean;
}) {
  const height = () => 480 - (props.badge ? COMPARE_HEADER : 0);
  return (
    <div
      class="absolute top-0 bottom-0 flex flex-col"
      style={{ left: `${props.left}px`, width: `${props.width}px`, background: PANEL }}
    >
      <Show when={props.badge}>
        <div
          class={`flex shrink-0 items-center justify-center text-[20px] ${LIGHT}`}
          style={{
            height: `${COMPARE_HEADER}px`,
            'box-shadow': props.active ? 'inset 0 -4px 0 var(--accent)' : undefined,
            color: props.active ? '#fff' : 'var(--color-secondary)',
          }}
        >
          {props.badge}
        </div>
      </Show>
      <div class="relative flex-1 overflow-hidden">
        <div style={{ transform: `translateY(${-scrollFor(props.focus, height())}px)` }}>
          <For each={SETTINGS}>
            {(setting, i) => (
              <SettingRow
                setting={setting}
                recipe={props.recipe}
                focused={i() === props.focus && props.active !== false}
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
        class={`flex items-center justify-between px-[14px] text-[24px] ${LIGHT}`}
        style={{ height: `${ROW}px`, background: CARD }}
      >
        <span class="truncate">{props.title}</span>
        <span class="tabular-nums">{props.value}</span>
      </div>
      <div class="px-[14px] py-[12px]" style={{ background: CARD }}>
        {props.children}
      </div>
    </div>
  );
}

/* ---- The editor screen ------------------------------------------------------- */

type Fly = 'none' | 'shift' | 'film';
type Mode = 'live' | 'raw';
type Side = 'a' | 'b';

interface EditorState {
  mode: Mode;
  /** B: the recipe being edited. */
  recipe: Recipe;
  focus: number;
  fly: Fly;
  /** A: the recipe kept to compare against. */
  snapshot?: Recipe;
  comparing: boolean;
  /** The compare column that takes edits. */
  side: Side;
  name: string;
}

function Hint(props: { keys: string; label: string }) {
  return (
    <span class="flex items-center gap-[6px] [text-shadow:0_1px_3px_rgb(0_0_0/0.9)]">
      <Key>{props.keys}</Key>
      <span class={`text-[17px] ${LIGHT}`}>{props.label}</span>
    </span>
  );
}

function Editor(props: { state: EditorState }) {
  const s = () => props.state;
  const focused = () => SETTINGS[s().focus];
  const rowTop = () => s().focus * ROW - scrollFor(s().focus, 480);
  const colors = palette(null);
  const split = 360;
  const compare = () => (s().comparing ? s().snapshot : undefined);

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
      <Show when={compare()}>
        {(a) => (
          <>
            <div class="absolute inset-y-0 left-0 overflow-hidden" style={{ width: `${split}px` }}>
              <div class="absolute inset-y-0 left-0 w-screen-w">
                <Scene recipe={a()} />
              </div>
            </div>
            <div class="absolute inset-y-0 w-[2px] bg-white" style={{ left: `${split - 1}px` }} />
            <Column
              recipe={a()}
              other={s().recipe}
              focus={s().focus}
              left={0}
              width={COMPARE_WIDTH}
              badge="A"
              active={s().side === 'a'}
            />
            <Column
              recipe={s().recipe}
              other={a()}
              focus={s().focus}
              left={720 - COMPARE_WIDTH}
              width={COMPARE_WIDTH}
              badge="B"
              active={s().side === 'b'}
            />
          </>
        )}
      </Show>

      <Show when={!compare()}>
        <Column recipe={s().recipe} other={s().snapshot} focus={s().focus} left={0} width={WIDTH} />
        <Show when={s().mode === 'raw' && s().fly === 'none'}>
          <div
            class="absolute top-[10px] right-[12px] flex items-center gap-[8px] px-[12px] py-[5px]"
            style={{ background: CARD }}
          >
            <span class="bg-(--accent) px-[6px] text-[15px] font-bold">RAW</span>
            <span class={`text-[20px] ${LIGHT}`}>{s().name}</span>
          </div>
        </Show>
        <Show when={s().fly === 'film'}>
          <Flyout row={rowTop()} left={WIDTH} title={FILMS[s().recipe.film].name}>
            <p class="text-[22px] leading-[30px] font-light tracking-[0.04em]">
              {FILMS[s().recipe.film].note}
            </p>
          </Flyout>
        </Show>
        <Show when={s().fly === 'shift'}>
          <Flyout
            row={Math.min(rowTop(), 480 - 12 - ROW - 4 - 298)}
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
        class={`absolute bottom-[10px] flex gap-[14px] ${compare() ? 'flex-col items-start' : 'right-[14px] items-center'}`}
        style={compare() ? { left: `${COMPARE_WIDTH + 10}px` } : undefined}
      >
        <Show when={!compare() && focused().id === 'wb' && s().fly === 'none'}>
          <Hint keys="▲" label="SHIFT" />
        </Show>
        <Show when={!compare() && focused().id === 'film' && s().fly === 'none'}>
          <Hint keys="DISP" label="INFO" />
        </Show>
        <Show when={compare()}>
          <Hint keys="AEL" label="A/B" />
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

/** Applies an edit to the column that has focus. */
function edit(state: EditorState, change: (r: Recipe) => Recipe): EditorState {
  if (state.comparing && state.side === 'a' && state.snapshot)
    return { ...state, snapshot: change(state.snapshot) };
  return { ...state, recipe: change(state.recipe) };
}

function step(state: EditorState, delta: number): EditorState {
  const setting = SETTINGS[state.focus];
  return edit(state, (r) => {
    const v = r[setting.id];
    const next =
      setting.kind === 'slider'
        ? Math.max(setting.min, Math.min(setting.max, v + delta))
        : Math.max(0, Math.min(setting.choices.length - 1, v + delta));
    return { ...r, [setting.id]: next };
  });
}

function onKey(state: EditorState, key: string): EditorState {
  const id = SETTINGS[state.focus].id;
  if (state.fly === 'shift') {
    const clamp = (n: number) => Math.max(-9, Math.min(9, n));
    if (key === 'ArrowLeft') return edit(state, (r) => ({ ...r, shiftR: clamp(r.shiftR - 1) }));
    if (key === 'ArrowRight') return edit(state, (r) => ({ ...r, shiftR: clamp(r.shiftR + 1) }));
    if (key === 'ArrowUp') return edit(state, (r) => ({ ...r, shiftB: clamp(r.shiftB + 1) }));
    if (key === 'ArrowDown') return edit(state, (r) => ({ ...r, shiftB: clamp(r.shiftB - 1) }));
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
  if (key === 'a') return { ...state, snapshot: { ...state.recipe }, comparing: false, side: 'b' };
  if (key === 'q' && state.snapshot)
    return { ...state, comparing: !state.comparing, fly: 'none', side: 'b' };
  if (key === 'Tab' && state.comparing) return { ...state, side: state.side === 'a' ? 'b' : 'a' };
  if (key === 'Escape') return { ...state, fly: 'none', comparing: false };
  return state;
}

const INITIAL: EditorState = {
  mode: 'live',
  recipe: BASE,
  focus: 3,
  fly: 'none',
  comparing: false,
  side: 'b',
  name: 'DSCF4182',
};

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
  { label: 'A/B', key: 'Tab' },
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
  film: 4,
  shadow: 2,
  highlight: 1,
  color: -1,
  grain: 2,
  shiftR: 4,
  shiftB: -5,
};

const STATES: { id: string; title: string; view: () => JSX.Element }[] = [
  { id: 'slider', title: 'Slider row', view: () => <Editor state={INITIAL} /> },
  { id: 'film', title: 'Film simulation', view: () => <Editor state={{ ...INITIAL, focus: 0 }} /> },
  {
    id: 'film-info',
    title: 'Film simulation notes',
    view: () => <Editor state={{ ...INITIAL, focus: 0, fly: 'film' }} />,
  },
  {
    id: 'wb',
    title: 'White balance',
    view: () => <Editor state={{ ...INITIAL, focus: 1, recipe: { ...BASE, wb: 4 } }} />,
  },
  {
    id: 'wb-shift',
    title: 'WB SHIFT',
    view: () => <Editor state={{ ...INITIAL, focus: 1, fly: 'shift' }} />,
  },
  {
    id: 'compare',
    title: 'Compare, editing B',
    view: () => (
      <Editor state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true }} />
    ),
  },
  {
    id: 'compare-a',
    title: 'Compare, editing A',
    view: () => (
      <Editor
        state={{ ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 0, comparing: true, side: 'a' }}
      />
    ),
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    view: () => <Editor state={{ ...INITIAL, mode: 'raw', recipe: EDITED, focus: 5 }} />,
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
