import { render } from '@solidjs/web';
import { For, Show, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { palette } from './palette';
import './focal.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Choice rows scroll their choices as stock icons,
   slider rows step through segments as FUJIMOD SET-UP's color bars do, and
   fly-outs (WB SHIFT, film notes) continue the focused row across the
   picture. Compare shows the column twice, A and B, scrolled together. The
   same screen serves RAW CONVERSION in playback.

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
  short: string;
  icon: RowIcon;
  min: number;
  max: number;
}
interface ChoiceSetting {
  kind: 'choice';
  id: string;
  name: string;
  short: string;
  icon: RowIcon;
  choices: Choice[];
}
type Setting = SliderSetting | ChoiceSetting;

const SETTINGS: Setting[] = [
  {
    kind: 'choice',
    id: 'film',
    name: 'FILM SIMULATION',
    short: 'FILM',
    icon: { stock: 0x5f3 },
    choices: FILMS,
  },
  {
    kind: 'choice',
    id: 'wb',
    name: 'WHITE BALANCE',
    short: 'WB',
    icon: { stock: 0x609 },
    choices: WB,
  },
  {
    kind: 'choice',
    id: 'dr',
    name: 'DYNAMIC RANGE',
    short: 'DR',
    icon: { stock: 0x5f1 },
    choices: DR,
  },
  {
    kind: 'slider',
    id: 'highlight',
    name: 'HIGHLIGHT TONE',
    short: 'H TONE',
    icon: { stock: 0x5f6 },
    min: -2,
    max: 2,
  },
  {
    kind: 'slider',
    id: 'shadow',
    name: 'SHADOW TONE',
    short: 'S TONE',
    icon: { stock: 0x607 },
    min: -2,
    max: 2,
  },
  {
    kind: 'slider',
    id: 'color',
    name: 'COLOR',
    short: 'COLOR',
    icon: { stock: 0x5f0 },
    min: -2,
    max: 2,
  },
  {
    kind: 'slider',
    id: 'sharp',
    name: 'SHARPNESS',
    short: 'SHARP',
    icon: { stock: 0x605 },
    min: -2,
    max: 2,
  },
  {
    kind: 'slider',
    id: 'nr',
    name: 'NOISE REDUCTION',
    short: 'NR',
    icon: { stock: 0x603 },
    min: -2,
    max: 2,
  },
  {
    kind: 'choice',
    id: 'chrome',
    name: 'COLOR CHROME EFFECT',
    short: 'CHROME',
    icon: { mod: 'color_chrome' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'fxblue',
    name: 'COLOR CHROME FX BLUE',
    short: 'FX BLUE',
    icon: { mod: 'color_chrome_blue' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'grain',
    name: 'GRAIN EFFECT',
    short: 'GRAIN',
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

/** The emulator's default scene, as walkthroughs show it. */
const PHOTO = new URL('../../engine/assets/default-scene.png', import.meta.url).href;

function Photo(props: { recipe: Recipe }) {
  return (
    <img
      alt=""
      src={PHOTO}
      class="absolute inset-0 h-full w-full object-cover"
      style={{ filter: look(props.recipe) }}
    />
  );
}

/* ---- Layout ------------------------------------------------------------------ */

/** Three ways to name the settings, to compare. */
type Names = 'title' | 'short' | 'tab';

const ROW = 46;
const COMPARE_HEADER = 30;
const ICON_CELL = 46;
const TEXT_CELL = 76;
const SHORT_LABEL = 84;

const widthFor = (names: Names) => (names === 'short' ? 300 : 280);

/** The same scroll for every column, so A and B stay in lockstep. */
function scrollFor(focus: number, height: number): number {
  const max = Math.max(0, SETTINGS.length * ROW - height);
  return Math.max(0, Math.min(max, focus * ROW - (height - ROW) / 2));
}

/** Flat translucent panels, as the stock live pickers draw them. */
const PANEL = 'rgb(20 20 20 / 0.8)';
const BODY = 'rgb(40 40 42 / 0.92)';
const LIGHT = 'font-light tracking-[0.08em] uppercase';
/** FUJIMOD SET-UP's selection: a white outline just outside the cell. */
const SELECTED = 'outline-[3px] outline-offset-[2px] outline-white';

/* ---- Input ------------------------------------------------------------------- */

type Side = 'a' | 'b';
type Action =
  | { key: string }
  | { focus: number; side?: Side }
  | { set: string; value: number; focus: number; side?: Side }
  | { shift: [number, number] };
type Send = (action: Action) => void;

/* ---- Rows -------------------------------------------------------------------- */

function RowIconView(props: { icon: RowIcon }) {
  return (
    <img
      alt=""
      class="h-[26px] w-[42px] shrink-0 object-contain"
      style={{ filter: 'invert(1)' }}
      src={'stock' in props.icon ? stock(props.icon.stock) : `${MOD}/${props.icon.mod}.svg`}
    />
  );
}

/** Every choice in a row, scrolled to keep the chosen one in view. */
function ChoiceStrip(props: {
  setting: ChoiceSetting;
  value: number;
  width: number;
  focused: boolean;
  pick: (value: number) => void;
}) {
  const cell = () => (props.setting.choices[0].icon ? ICON_CELL : TEXT_CELL);
  const offset = () => {
    const max = Math.max(0, props.setting.choices.length * cell() - props.width);
    return Math.max(0, Math.min(max, props.value * cell() - (props.width - cell()) / 2));
  };
  return (
    <div class="relative h-[36px] overflow-hidden" style={{ width: `${props.width}px` }}>
      <div
        class="absolute inset-y-0 left-0 flex items-center"
        style={{ transform: `translateX(${-offset()}px)` }}
      >
        <For each={props.setting.choices.map((choice, i) => ({ choice, i }))}>
          {(item) => {
            const chosen = () => item.i === props.value;
            return (
              <div
                class="flex shrink-0 items-center justify-center px-[3px]"
                style={{ width: `${cell()}px` }}
                onClick={(e) => {
                  e.stopPropagation();
                  props.pick(item.i);
                }}
              >
                <div
                  class={`flex h-[28px] w-full items-center justify-center ${chosen() && props.focused ? SELECTED : ''}`}
                  style={{
                    background: chosen() ? 'rgb(255 255 255 / 0.16)' : undefined,
                    opacity: chosen() ? 1 : 0.6,
                  }}
                >
                  <Show
                    when={item.choice.icon}
                    fallback={<span class={`text-[15px] ${LIGHT}`}>{item.choice.name}</span>}
                  >
                    {(icon) => (
                      <img alt="" class="h-[24px] w-[38px] object-contain" src={stock(icon())} />
                    )}
                  </Show>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

/** One segment per step, as FUJIMOD SET-UP's color bars: zero to the value
 * filled, the value outlined while the row has focus. */
function Segments(props: {
  setting: SliderSetting;
  value: number;
  focused: boolean;
  pick: (value: number) => void;
}) {
  const steps = () =>
    Array.from(
      { length: props.setting.max - props.setting.min + 1 },
      (_, i) => props.setting.min + i,
    );
  const filled = (v: number) =>
    (props.value > 0 && v > 0 && v <= props.value) ||
    (props.value < 0 && v < 0 && v >= props.value);
  return (
    <div class="flex h-[22px] flex-1 gap-[3px]">
      <For each={steps()}>
        {(v) => (
          <span
            class={`relative flex-1 border border-black ${v === props.value && props.focused ? SELECTED : ''}`}
            style={{
              background: filled(v)
                ? 'var(--accent)'
                : v === props.value
                  ? '#d8d8dc'
                  : 'rgb(120 120 126 / 0.7)',
            }}
            onClick={(e) => {
              e.stopPropagation();
              props.pick(v);
            }}
          >
            <Show when={v === 0}>
              <span class="absolute inset-y-[5px] left-1/2 w-[2px] -translate-x-1/2 bg-black/60" />
            </Show>
          </span>
        )}
      </For>
    </div>
  );
}

function SettingRow(props: {
  setting: Setting;
  index: number;
  recipe: Recipe;
  focused: boolean;
  changed: boolean;
  width: number;
  names: Names;
  side?: Side;
  send: Send;
}) {
  const lead = () => (props.names === 'short' ? SHORT_LABEL : 42);
  const controlWidth = () => props.width - 12 - lead() - 10 - 10;
  const pick = (value: number) =>
    props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  return (
    <div
      class={`relative flex items-center gap-[10px] pr-[10px] pl-[12px] ${props.focused ? 'focal-focused' : 'focal-bevel'}`}
      style={{ height: `${ROW}px` }}
      onClick={() => props.send({ focus: props.index, side: props.side })}
    >
      <Show when={props.names === 'short'} fallback={<RowIconView icon={props.setting.icon} />}>
        <span
          class={`shrink-0 truncate text-[17px] ${LIGHT}`}
          style={{ width: `${SHORT_LABEL}px` }}
        >
          {props.setting.short}
        </span>
      </Show>
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <ChoiceStrip
            setting={props.setting as ChoiceSetting}
            value={props.recipe[props.setting.id]}
            width={controlWidth()}
            focused={props.focused}
            pick={pick}
          />
        }
      >
        {(slider) => (
          <>
            <Segments
              setting={slider()}
              value={props.recipe[slider().id]}
              focused={props.focused}
              pick={pick}
            />
            <span
              class="w-[34px] text-right text-[22px] leading-none tabular-nums"
              style={{ color: props.changed ? 'var(--accent-text)' : undefined }}
            >
              {fmt(props.recipe[slider().id])}
            </span>
          </>
        )}
      </Show>
      <Show when={props.changed && props.setting.kind === 'choice'}>
        <span class="absolute top-[4px] right-[4px] h-[6px] w-[6px] bg-(--accent)" />
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
  names: Names;
  side?: Side;
  active?: boolean;
  send: Send;
}) {
  const height = () => 480 - (props.side ? COMPARE_HEADER : 0);
  return (
    <div
      class="absolute top-0 bottom-0 flex flex-col"
      style={{ left: `${props.left}px`, width: `${props.width}px`, background: PANEL }}
    >
      <Show when={props.side}>
        <div
          class={`flex shrink-0 items-center justify-center text-[20px] ${LIGHT}`}
          style={{
            height: `${COMPARE_HEADER}px`,
            'box-shadow': props.active ? 'inset 0 -3px 0 var(--accent)' : undefined,
            color: props.active ? '#fff' : 'var(--color-secondary)',
          }}
          onClick={() => props.send({ focus: props.focus, side: props.side })}
        >
          {props.side === 'a' ? 'A' : 'B'}
        </div>
      </Show>
      <div class="relative flex-1 overflow-hidden">
        <div style={{ transform: `translateY(${-scrollFor(props.focus, height())}px)` }}>
          <For each={SETTINGS}>
            {(setting, i) => (
              <SettingRow
                setting={setting}
                index={i()}
                recipe={props.recipe}
                focused={i() === props.focus && props.active !== false}
                changed={
                  props.other !== undefined && props.other[setting.id] !== props.recipe[setting.id]
                }
                width={props.width}
                names={props.names}
                side={props.side}
                send={props.send}
              />
            )}
          </For>
        </div>
      </div>
    </div>
  );
}

/* ---- Fly-outs ---------------------------------------------------------------- */

/** The focused row continued across the picture: its focus bar runs on as the
 * fly-out's title band, and the body hangs from it (or rises above it when it
 * would leave the screen). */
function RowExtension(props: {
  top: number;
  left: number;
  title: string;
  value?: string;
  bodyHeight?: number;
  children?: JSX.Element;
  onClick?: () => void;
}) {
  const below = () => props.top + ROW + (props.bodyHeight ?? 0) <= 480 - 8;
  return (
    <>
      <div
        class={`absolute flex items-center justify-between pr-[14px] pl-[10px] text-[20px] ${LIGHT}`}
        style={{
          left: `${props.left}px`,
          right: '0',
          top: `${props.top}px`,
          height: `${ROW}px`,
          background: 'var(--focus)',
          'box-shadow': 'inset 0 1px 0 var(--hairline), inset 0 -2px 0 #000',
        }}
        onClick={() => props.onClick?.()}
      >
        <span class="truncate">{props.title}</span>
        <span class="tabular-nums">{props.value}</span>
      </div>
      <Show when={props.children}>
        <div
          class="absolute px-[14px] py-[12px]"
          style={{
            left: `${props.left}px`,
            right: '0',
            height: `${props.bodyHeight}px`,
            top: below() ? `${props.top + ROW}px` : `${props.top - (props.bodyHeight ?? 0)}px`,
            background: BODY,
          }}
        >
          {props.children}
        </div>
      </Show>
    </>
  );
}

/** WB SHIFT: R on the horizontal axis, B on the vertical, -9 to +9 as stock. */
function ShiftGrid(props: { r: number; b: number; send: Send }) {
  const cell = 11;
  const size = cell * 19;
  const pos = (v: number) => (v + 9) * cell;
  return (
    <div
      class="relative"
      style={{ width: `${size}px`, height: `${size}px` }}
      onClick={(e) => {
        const box = e.currentTarget.getBoundingClientRect();
        const scale = box.width / size;
        const r = Math.floor((e.clientX - box.left) / scale / cell) - 9;
        const b = 9 - Math.floor((e.clientY - box.top) / scale / cell);
        props.send({ shift: [Math.max(-9, Math.min(9, r)), Math.max(-9, Math.min(9, b))] });
      }}
    >
      <div
        class="absolute inset-0"
        style={{
          background:
            'linear-gradient(to right, rgb(60 200 120 / 0.6), transparent 50%, rgb(240 70 80 / 0.6)), linear-gradient(to top, rgb(240 200 60 / 0.6), transparent 50%, rgb(60 110 255 / 0.6))',
        }}
      />
      <div class="absolute inset-y-0 left-1/2 w-[1px] bg-white/60" />
      <div class="absolute inset-x-0 top-1/2 h-[1px] bg-white/60" />
      <div
        class={`absolute ${SELECTED}`}
        style={{
          left: `${pos(props.r)}px`,
          top: `${pos(-props.b)}px`,
          width: `${cell}px`,
          height: `${cell}px`,
          background: '#fff',
        }}
      />
      <span class="absolute -top-[4px] left-1/2 -translate-x-1/2 -translate-y-full text-[15px]">
        B
      </span>
      <span class="absolute top-1/2 -right-[6px] translate-x-full -translate-y-1/2 text-[15px]">
        R
      </span>
      <span class="absolute -bottom-[4px] left-1/2 -translate-x-1/2 translate-y-full text-[15px]">
        Y
      </span>
      <span class="absolute top-1/2 -left-[6px] -translate-x-full -translate-y-1/2 text-[15px]">
        G
      </span>
    </div>
  );
}

/* ---- Legend ------------------------------------------------------------------ */

/** The stock floating legend: one dark box per key, the key glyph inverted. */
function Legend(props: {
  hints: { glyph: string; label: string; key: string }[];
  send: Send;
  left: number;
}) {
  return (
    <div class="absolute bottom-[12px] flex gap-[4px]" style={{ left: `${props.left}px` }}>
      <For each={props.hints}>
        {(hint) => (
          <div
            class="flex h-[32px] items-center gap-[8px] px-[6px] whitespace-nowrap"
            style={{ background: 'rgb(0 0 0 / 0.6)' }}
            onClick={() => props.send({ key: hint.key })}
          >
            <span class="flex h-[22px] min-w-[22px] items-center justify-center bg-white px-[3px] text-[14px] leading-none font-semibold text-black">
              {hint.glyph}
            </span>
            <span class="pr-[4px] text-[17px] font-light tracking-[0.16em] uppercase">
              {hint.label}
            </span>
          </div>
        )}
      </For>
    </div>
  );
}

/* ---- The editor screen ------------------------------------------------------- */

type Fly = 'none' | 'shift' | 'film';
type Mode = 'live' | 'raw';

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
  names: Names;
}

function choiceName(setting: Setting, recipe: Recipe): string | undefined {
  return setting.kind === 'choice' ? setting.choices[recipe[setting.id]].name : undefined;
}

function Editor(props: { state: EditorState; send?: Send }) {
  const s = () => props.state;
  const send: Send = (action) => props.send?.(action);
  const focused = () => SETTINGS[s().focus];
  const width = () => widthFor(s().names);
  const rowTop = () => s().focus * ROW - scrollFor(s().focus, 480);
  const colors = palette(null);
  const compare = () => (s().comparing ? s().snapshot : undefined);
  const compareWidth = 300;

  const hints = () => {
    const list: { glyph: string; label: string; key: string }[] = [];
    if (compare()) list.push({ glyph: 'AEL', label: 'A/B', key: 'Tab' });
    else if (focused().id === 'wb' && s().fly === 'none')
      list.push({ glyph: '▲', label: 'SHIFT', key: 's' });
    else if (focused().id === 'film' && s().fly === 'none')
      list.push({ glyph: 'DISP', label: 'INFO', key: 'i' });
    if (s().snapshot) list.push({ glyph: 'Q', label: s().comparing ? 'HIDE A' : 'A|B', key: 'q' });
    else list.push({ glyph: 'AF-ON', label: 'KEEP A', key: 'a' });
    list.push({ glyph: 'OK', label: s().mode === 'raw' ? 'CREATE' : 'SAVE', key: 'Enter' });
    return list;
  };

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
      <Photo recipe={s().recipe} />
      <Show when={compare()}>
        {(a) => (
          <>
            <div class="absolute inset-y-0 left-0 w-[360px] overflow-hidden">
              <div class="absolute inset-y-0 left-0 w-screen-w">
                <Photo recipe={a()} />
              </div>
            </div>
            <div class="absolute inset-y-0 left-[359px] w-[2px] bg-white" />
            <For each={['a', 'b'] as Side[]}>
              {(side) => (
                <Column
                  recipe={side === 'a' ? a() : s().recipe}
                  other={side === 'a' ? s().recipe : a()}
                  focus={s().focus}
                  left={side === 'a' ? 0 : 720 - compareWidth}
                  width={compareWidth}
                  names={s().names}
                  side={side}
                  active={s().side === side}
                  send={send}
                />
              )}
            </For>
          </>
        )}
      </Show>

      <Show when={!compare()}>
        <Column
          recipe={s().recipe}
          other={s().snapshot}
          focus={s().focus}
          left={0}
          width={width()}
          names={s().names}
          send={send}
        />
        <Show when={s().names === 'title' && s().fly === 'none'}>
          <div
            class="absolute top-[10px] flex flex-col items-start gap-[4px]"
            style={{ left: `${width() + 12}px` }}
          >
            <span
              class={`px-[12px] py-[5px] text-[24px] ${LIGHT}`}
              style={{ background: 'rgb(60 60 60 / 0.85)' }}
            >
              {focused().name}
            </span>
            <Show when={choiceName(focused(), s().recipe)}>
              {(name) => (
                <span
                  class={`px-[12px] py-[4px] text-[18px] ${LIGHT}`}
                  style={{ background: 'rgb(60 60 60 / 0.85)' }}
                >
                  {name()}
                </span>
              )}
            </Show>
          </div>
        </Show>
        <Show when={s().names === 'tab' && s().fly === 'none'}>
          <RowExtension
            top={rowTop()}
            left={width()}
            title={focused().name}
            value={choiceName(focused(), s().recipe)}
            onClick={() =>
              send({ key: focused().id === 'film' ? 'i' : focused().id === 'wb' ? 's' : '' })
            }
          />
        </Show>
        <Show when={s().mode === 'raw'}>
          <span class="absolute top-[10px] right-[12px] bg-(--accent) px-[8px] py-[2px] text-[17px] font-bold">
            RAW
          </span>
        </Show>
        <Show when={s().fly === 'film'}>
          <RowExtension
            top={rowTop()}
            left={width()}
            title={FILMS[s().recipe.film].name}
            bodyHeight={112}
            onClick={() => send({ key: 'i' })}
          >
            <p class="text-[21px] leading-[29px] font-light tracking-[0.04em]">
              {FILMS[s().recipe.film].note}
            </p>
          </RowExtension>
        </Show>
        <Show when={s().fly === 'shift'}>
          <RowExtension
            top={rowTop()}
            left={width()}
            title="WB SHIFT"
            value={`R${fmt(s().recipe.shiftR)} B${fmt(s().recipe.shiftB)}`}
            bodyHeight={270}
            onClick={() => send({ key: 'Escape' })}
          >
            <div class="flex h-full items-center justify-center">
              <ShiftGrid r={s().recipe.shiftR} b={s().recipe.shiftB} send={send} />
            </div>
          </RowExtension>
        </Show>
      </Show>

      <Show when={s().fly !== 'shift'}>
        <Legend hints={hints()} send={send} left={compare() ? 312 : width() + 12} />
      </Show>
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

function apply(state: EditorState, action: Action): EditorState {
  if ('key' in action) return onKey(state, action.key);
  if ('shift' in action)
    return edit(state, (r) => ({ ...r, shiftR: action.shift[0], shiftB: action.shift[1] }));
  const moved = {
    ...state,
    focus: action.focus,
    side: action.side ?? state.side,
    fly: action.focus === state.focus ? state.fly : ('none' as Fly),
  };
  if ('set' in action) return edit(moved, (r) => ({ ...r, [action.set]: action.value }));
  return moved;
}

const INITIAL: EditorState = {
  mode: 'live',
  recipe: BASE,
  focus: 3,
  fly: 'none',
  comparing: false,
  side: 'b',
  name: 'DSCF4182',
  names: 'tab',
};

const PADS = [
  { label: '◀', key: 'ArrowLeft' },
  { label: '▲', key: 'ArrowUp' },
  { label: '▼', key: 'ArrowDown' },
  { label: '▶', key: 'ArrowRight' },
  { label: 'OK', key: 'Enter' },
  { label: 'BACK', key: 'Escape' },
];

const NAMES: { names: Names; label: string }[] = [
  { names: 'tab', label: 'Name tab' },
  { names: 'title', label: 'Title box' },
  { names: 'short', label: 'Short names' },
];

function Live() {
  const [state, setState] = createSignal<EditorState>(INITIAL);
  const send: Send = (action) => setState(apply(state(), action));
  return (
    <div
      tabindex="0"
      class="outline-none"
      onKeyDown={(e) => {
        const next = onKey(state(), e.key);
        if (next !== state()) {
          e.preventDefault();
          setState(next);
        }
      }}
    >
      <div class="mb-[10px] flex flex-wrap gap-[8px]">
        <For each={NAMES}>
          {(choice) => (
            <button
              type="button"
              class="min-h-[40px] rounded-[10px] px-[12px] text-[15px] font-semibold"
              style={{ background: state().names === choice.names ? '#0068d8' : '#2a2a2c' }}
              onClick={() => setState({ ...state(), names: choice.names })}
            >
              {choice.label}
            </button>
          )}
        </For>
      </div>
      <Fit>
        <Editor state={state()} send={send} />
      </Fit>
      <div class="mt-[10px] flex max-w-[720px] flex-wrap gap-[8px]">
        <For each={PADS}>
          {(pad) => (
            <button
              type="button"
              class="min-h-[44px] min-w-[52px] rounded-[10px] bg-[#2a2a2c] px-[12px] text-[16px] font-semibold active:bg-[#444]"
              onClick={() => send({ key: pad.key })}
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

const STATES: { id: string; title: string; state: EditorState }[] = [
  { id: 'tab', title: 'Name tab on the focused row', state: { ...INITIAL, focus: 1 } },
  {
    id: 'title',
    title: 'Title box, as the stock pickers',
    state: { ...INITIAL, focus: 1, names: 'title' },
  },
  {
    id: 'short',
    title: 'Short names instead of icons',
    state: { ...INITIAL, focus: 1, names: 'short' },
  },
  { id: 'slider', title: 'Slider row', state: { ...INITIAL, focus: 4 } },
  { id: 'film-info', title: 'Film simulation notes', state: { ...INITIAL, focus: 0, fly: 'film' } },
  { id: 'wb-shift', title: 'WB SHIFT', state: { ...INITIAL, focus: 1, fly: 'shift' } },
  {
    id: 'compare',
    title: 'Compare, editing B',
    state: { ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true },
  },
  {
    id: 'compare-a',
    title: 'Compare, editing A',
    state: { ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 0, comparing: true, side: 'a' },
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    state: { ...INITIAL, mode: 'raw', recipe: EDITED, focus: 5 },
  },
];

function Review() {
  return (
    <div class="min-h-dvh bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
      <h1 class="mb-[12px] text-[22px] font-semibold">Recipe editor</h1>
      <Live />
      <For each={STATES}>
        {(entry) => (
          <section id={entry.id} class="mt-[32px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h2>
            <Fit>
              <Editor state={entry.state} />
            </Fit>
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
