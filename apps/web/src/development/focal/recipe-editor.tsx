import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Choice, Dial, type ChoiceItem } from './controls';
import { COLUMN, GLASS } from './materials';
import { palette } from './palette';
import './focal.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Choice rows scroll their choices as stock icons,
   slider rows step through segments as FUJIMOD SET-UP's color bars do, and
   the WB SHIFT fly-out continues the focused row across the
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

export const FILMS: Choice[] = [
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
const GRAIN_SIZE: Choice[] = [{ name: 'SMALL' }, { name: 'LARGE' }];
const PRIORITY: Choice[] = [{ name: 'OFF' }, { name: 'WEAK' }, { name: 'STRONG' }, { name: 'AUTO' }];
const CLARITY: Step[] = [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5];

/** A row's icon: a stock Q menu vector, a mod's own artwork, or a word on a
 * stock-style plate with an optional badge letter. */
type RowIcon = { stock: number } | { mod: string } | { word: string; badge?: string };

/** A slider's steps in order; numbers show signed, words as written. */
type Step = number | string;
const LEVELS: Step[] = [-2, -1, 0, 1, 2];
/** The noise-reduction mod's levels below -2, then stock's. */
const NR_LEVELS: Step[] = ['OFF', 'SNR', 'CNR', 'LNR', -2, -1, 0, 1, 2];
/** The noise-reduction mod's full names, for the title box. */
const STEP_NAMES: Record<string, string> = {
  OFF: 'NR OFF',
  SNR: 'SPECKLE NR ONLY',
  CNR: 'COLOR NR LOW',
  LNR: 'LUMA NR OFF',
};

interface SliderSetting {
  kind: 'slider';
  id: string;
  name: string;
  icon: RowIcon;
  /** A second dimension of the setting above, sharing its icon. */
  continues?: boolean;
  steps: Step[];
}
interface ChoiceSetting {
  kind: 'choice';
  id: string;
  name: string;
  icon: RowIcon;
  continues?: boolean;
  choices: Choice[];
}
type Setting = SliderSetting | ChoiceSetting;

const SETTINGS: Setting[] = [
  { kind: 'choice', id: 'film', name: 'FILM SIMULATION', icon: { stock: 0x5f3 }, choices: FILMS },
  { kind: 'choice', id: 'wb', name: 'WHITE BALANCE', icon: { stock: 0x609 }, choices: WB },
  { kind: 'choice', id: 'dr', name: 'DYNAMIC RANGE', icon: { stock: 0x5f1 }, choices: DR },
  {
    kind: 'choice',
    id: 'priority',
    name: 'D RANGE PRIORITY',
    icon: { mod: 'd_range_priority' },
    choices: PRIORITY,
  },
  {
    kind: 'slider',
    id: 'highlight',
    name: 'HIGHLIGHT TONE',
    icon: { stock: 0x5f6 },
    steps: LEVELS,
  },
  { kind: 'slider', id: 'shadow', name: 'SHADOW TONE', icon: { stock: 0x607 }, steps: LEVELS },
  { kind: 'slider', id: 'color', name: 'COLOR', icon: { stock: 0x5f0 }, steps: LEVELS },
  { kind: 'slider', id: 'sharp', name: 'SHARPNESS', icon: { stock: 0x605 }, steps: LEVELS },
  { kind: 'slider', id: 'clarity', name: 'CLARITY', icon: { mod: 'clarity' }, steps: CLARITY },
  { kind: 'slider', id: 'nr', name: 'NOISE REDUCTION', icon: { stock: 0x603 }, steps: NR_LEVELS },
  {
    kind: 'choice',
    id: 'chrome',
    name: 'COLOR CHROME EFFECT',
    icon: { word: 'Chrome' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'fxblue',
    name: 'COLOR CHROME FX BLUE',
    icon: { word: 'Chrome', badge: 'B' },
    choices: EFFECT,
  },
  {
    kind: 'choice',
    id: 'grain',
    name: 'GRAIN EFFECT',
    icon: { mod: 'film_grain' },
    choices: GRAIN,
  },
  {
    kind: 'choice',
    id: 'grainSize',
    name: 'GRAIN SIZE',
    icon: { mod: 'film_grain' },
    continues: true,
    choices: GRAIN_SIZE,
  },
];

/** Rows under one icon: a setting and the further dimensions that continue it. */
const GROUPS: number[][] = SETTINGS.reduce<number[][]>((groups, setting, i) => {
  if (setting.continues && groups.length) groups[groups.length - 1].push(i);
  else groups.push([i]);
  return groups;
}, []);

/** Every setting holds the index of its step or choice; WB SHIFT holds R and B. */
type Recipe = Record<string, number> & { shiftR: number; shiftB: number };

const at = (steps: Step[], value: Step) => steps.indexOf(value);
const BASE: Recipe = {
  film: 3,
  wb: 0,
  dr: 0,
  priority: 0,
  clarity: at(CLARITY, 0),
  highlight: at(LEVELS, -1),
  shadow: at(LEVELS, 1),
  color: at(LEVELS, 2),
  sharp: at(LEVELS, -1),
  nr: at(NR_LEVELS, -2),
  chrome: 1,
  fxblue: 0,
  grain: 1,
  grainSize: 0,
  shiftR: 2,
  shiftB: -3,
};

const fmt = (n: Step) =>
  typeof n === 'string' ? n : n > 0 ? `+${n}` : n < 0 ? `\u2212${-n}` : '0';
const level = (r: Recipe, id: string) => {
  const step = LEVELS[r[id]];
  return typeof step === 'number' ? step : 0;
};

/** An approximation of the recipe's look as CSS filters, enough to see edits move. */
function look(r: Recipe): string {
  const film = r.film;
  const mono = film >= 6 && film <= 9;
  const sepia = film === 10;
  const sat = [1, 1.45, 0.95, 0.72, 1.05, 0.9][film] ?? 0;
  const contrast =
    1 + (level(r, 'shadow') + level(r, 'highlight')) * 0.07 + (film === 4 ? 0.08 : 0);
  const warm = r.shiftR * 2 - r.shiftB * 2 + ([0, 0, 4, 0, 10, -14, -14, -14, -24, 2][r.wb] ?? 0);
  return [
    `saturate(${mono || sepia ? 0 : sat * (1 + level(r, 'color') * 0.15) * (1 + r.chrome * 0.04)})`,
    `contrast(${contrast + (film === 8 ? 0.15 : 0)})`,
    `brightness(${1 - level(r, 'shadow') * 0.03 + 0.02})`,
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

/* ---- Designs ----------------------------------------------------------------- */




interface Card {
  style: JSX.CSSProperties;
  class: string;
}

interface Design {
  label: string;
  panel: JSX.CSSProperties;
  /** The column's edge on the side that meets the picture. */
  edge: (side: 'left' | 'right') => string | undefined;
  row: (focused: boolean) => JSX.CSSProperties;
  /** The row's own top and bottom lines, which a drawer's contents stay between. */
  lines: { top: number; bottom: number; right?: number };
  /** How the design writes setting and choice names. */
  text?: (name: string) => string;
  /** How the focused row is marked, beyond its own surface. */
  focus: 'stripe' | 'none';
  focusColor: string;
  iconInk: (focused: boolean) => string;
  /** The stroke after the icon column. */
  divider?: (focused: boolean) => string | undefined;
  card: Card;
  legend: { box: JSX.CSSProperties; key: JSX.CSSProperties };
  /** The fly-out's body below its band; the band is the focused row. */
  body: JSX.CSSProperties;
}

const ACCENT = 'var(--accent)';

/** The film editor's design language (materials.ts): rows as cards in the
 * column, the chosen choice a white plate, a scale's ticks under a fixed
 * mark, chips over the picture. */
const DESIGN: Design = {
  label: 'Focal',
  panel: COLUMN,
  edge: () => 'var(--m-column-edge)',
  row: () => ({}),
  lines: { top: 3, bottom: 3, right: 6 },
  focus: 'none',
  focusColor: ACCENT,
  iconInk: (focused) => (focused ? '#fff' : 'rgb(255 255 255 / 0.7)'),
  card: { style: { ...GLASS, 'border-radius': '12px' }, class: 'font-semibold' },
  legend: {
    box: { ...GLASS, 'border-radius': '12px' },
    key: { background: '#fff', color: '#000', 'border-radius': '6px' },
  },
  body: { ...GLASS, 'border-radius': '16px' },
};

/* ---- Layout ------------------------------------------------------------------ */

/** The menu pages' row height. */
const ROW = 57;
/** The icon column, before the stroke that starts each row's drawer. */
const ICON_COLUMN = 58;
const WIDTH = 280;
const COMPARE_WIDTH = 300;
const STRIPE = 4;

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
export type Send = (action: Action) => void;

/* ---- Rows -------------------------------------------------------------------- */

/** Stock icons drawn as the Q tiles draw them: the ink in one color, its
 * counters cut through to the surface beneath. */
function Ink(props: { src: string; class: string; color: string }) {
  return (
    <span
      class={`shrink-0 ${props.class}`}
      style={{
        '-webkit-mask': `url(${props.src}) center / contain no-repeat`,
        mask: `url(${props.src}) center / contain no-repeat`,
        'mask-mode': 'alpha',
        'background-color': props.color,
      }}
    />
  );
}

/** A word on a plate in the icon ink, letters cut through, as the stock
 * Color and Sharp icons are; a badge letter on its lower right corner. */
function Wordmark(props: { word: string; badge?: string; color: string }) {
  return (
    <span
      class="relative flex h-[30px] w-[39px] items-center justify-center overflow-visible rounded-[3px]"
      style={{ background: props.color }}
    >
      <span
        class="inline-block text-[22px] leading-none font-bold tracking-[-0.03em] whitespace-nowrap"
        style={{ color: 'rgb(0 0 0 / 0.88)', transform: 'scaleX(0.46)' }}
      >
        {props.word}
      </span>
      <Show when={props.badge}>
        <span
          class="absolute -right-[8px] -bottom-[6px] flex h-[16px] w-[16px] items-center justify-center rounded-[3px] text-[12px] leading-none font-bold"
          style={{ background: props.color, color: '#000', 'box-shadow': '0 0 0 2px #000' }}
        >
          {props.badge}
        </span>
      </Show>
    </span>
  );
}

function RowIconView(props: { icon: RowIcon; color: string }) {
  const word = () => ('word' in props.icon ? props.icon : undefined);
  return (
    <Show
      when={!word()}
      fallback={<Wordmark word={word()!.word} badge={word()!.badge} color={props.color} />}
    >
    <Show
      when={'stock' in props.icon ? props.icon.stock : undefined}
      fallback={
        <Ink
          src={`${MOD}/${(props.icon as { mod: string }).mod}.svg`}
          class="-mx-[2px] h-[30px] w-[44px]"
          color={props.color}
        />
      }
    >
      {(id) => (
        // Q icons are 80 by 48 with their ink in the middle 52 by 40; at 0.75
        // the ink is the root menu's 39 by 30.
        <Ink src={stock(id())} class="-mx-[10px] h-[36px] w-[60px]" color={props.color} />
      )}
    </Show>
    </Show>
  );
}

/** One setting's controls, beside its group's icon. */
function SettingRow(props: {
  setting: Setting;
  index: number;
  recipe: Recipe;
  focused: boolean;
  changed: boolean;
  width: number;
  design: Design;
  side?: Side;
  send: Send;
}) {
  const d = () => props.design;
  const controlWidth = () => props.width - ICON_COLUMN - 1 - (d().lines.right ?? 0);
  const controlHeight = () => ROW - d().lines.top - d().lines.bottom;
  const pick = (value: number) =>
    props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  const focus = () => props.send({ focus: props.index, side: props.side });
  return (
    <div
      class="relative flex items-stretch"
      style={{
        height: `${ROW}px`,
        'padding-top': `${d().lines.top}px`,
        'padding-bottom': `${d().lines.bottom}px`,
        'padding-right': `${d().lines.right ?? 0}px`,
      }}
    >
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <Choice
            items={(props.setting as ChoiceSetting).choices.map((choice): ChoiceItem =>
              choice.icon === undefined
                ? choice.name
                : { name: choice.name, art: (ink) => <Ink src={stock(choice.icon!)} class="h-[24px] w-[46px]" color={ink} /> },
            )}
            value={props.recipe[props.setting.id]}
            width={controlWidth()}
            height={controlHeight()}
            focused={props.focused}
            changed={props.changed}
            pick={pick}
            onFocus={focus}
          />
        }
      >
        {(slider) => (
          <Dial
            value={props.recipe[slider().id]}
            min={0}
            max={slider().steps.length - 1}
            step={1}
            zero={Math.max(0, slider().steps.indexOf(0))}
            format={(i) => fmt(slider().steps[i])}
            width={controlWidth()}
            height={controlHeight()}
            focused={props.focused}
            changed={props.changed}
            set={pick}
            onFocus={focus}
          />
        )}
      </Show>
    </div>
  );
}

/** A setting's icon, once, beside the rows of every dimension it has; the
 * icon's cell takes the focus when any of them has it. */
function SettingGroup(props: {
  rows: number[];
  recipe: Recipe;
  other?: Recipe;
  focus?: number;
  width: number;
  design: Design;
  side?: Side;
  send: Send;
}) {
  const d = () => props.design;
  const at = () => props.rows.indexOf(props.focus ?? -1);
  return (
    <div class="relative flex">
      <div
        class="flex shrink-0 items-center justify-center"
        style={{
          width: `${ICON_COLUMN}px`,
          height: `${props.rows.length * ROW}px`,
          ...d().row(at() >= 0),
          'border-right': `1px solid ${d().divider?.(at() >= 0) ?? 'transparent'}`,
          'padding-bottom': `${d().lines.bottom - d().lines.top}px`,
        }}
        onClick={() => props.send({ focus: props.rows[Math.max(0, at())], side: props.side })}
      >
        <RowIconView icon={SETTINGS[props.rows[0]].icon} color={d().iconInk(at() >= 0)} />
      </div>
      <div class="flex min-w-0 flex-1 flex-col">
        <For each={props.rows}>
          {(index) => (
            <SettingRow
              setting={SETTINGS[index]}
              index={index}
              recipe={props.recipe}
              focused={index === props.focus}
              changed={
                props.other !== undefined &&
                props.other[SETTINGS[index].id] !== props.recipe[SETTINGS[index].id]
              }
              width={props.width}
              design={d()}
              side={props.side}
              send={props.send}
            />
          )}
        </For>
      </div>
      <Show when={at() >= 0 && d().focus === 'stripe'}>
        <span
          class="pointer-events-none absolute left-0"
          style={{
            top: `${at() * ROW + 1}px`,
            height: `${ROW - 3}px`,
            width: `${STRIPE}px`,
            background: d().focusColor,
          }}
        />
      </Show>
    </div>
  );
}

/** The settings column, a scroller of rows. In compare, the same rows render once for A and once for B. */
function Column(props: {
  recipe: Recipe;
  other?: Recipe;
  focus: number;
  left: number;
  width: number;
  design: Design;
  side?: Side;
  active?: boolean;
  /** Shared by A and B so they scroll together, and read by the fly-outs. */
  scroll: number;
  setScroll: (top: number) => void;
  send: Send;
}) {
  let scroller: HTMLDivElement | undefined;
  /* Another column (A or B) moved: follow it. */
  createEffect(
    () => props.scroll,
    (top) => {
      if (scroller && Math.abs(scroller.scrollTop - top) > 1) scroller.scrollTop = top;
    },
  );
  /* A key moved the focus: bring its row into view, smoothly. */
  let opened = false;
  createEffect(
    () => props.focus,
    (focus) => {
      if (!scroller || props.active === false) return;
      const top = focus * ROW;
      const bottom = top + ROW;
      const behavior = opened ? 'smooth' : 'auto';
      opened = true;
      if (top < scroller.scrollTop + ROW / 2) scroller.scrollTo({ top: top - ROW, behavior });
      else if (bottom > scroller.scrollTop + scroller.clientHeight - ROW / 2)
        scroller.scrollTo({ top: bottom - scroller.clientHeight + ROW, behavior });
    },
  );
  return (
    <div
      class="absolute inset-y-0 flex flex-col"
      style={{
        left: `${props.left}px`,
        width: `${props.width}px`,
        ...props.design.panel,
        'box-shadow': props.design.edge(props.side === 'b' ? 'left' : 'right'),
      }}
    >
      <div
        ref={(element) => (scroller = element)}
        class="relative flex-1 overflow-y-auto overscroll-contain"
        style={{ 'scrollbar-width': 'none' }}
        onScroll={(e) => props.setScroll(e.currentTarget.scrollTop)}
      >
        <For each={GROUPS}>
          {(rows) => (
            <SettingGroup
              rows={rows}
              recipe={props.recipe}
              other={props.other}
              focus={props.active !== false ? props.focus : undefined}
              width={props.width}
              design={props.design}
              side={props.side}
              send={props.send}
            />
          )}
        </For>
      </div>
    </div>
  );
}

/* ---- Fly-outs ---------------------------------------------------------------- */

/** The focused row continued past the column: its focus bar runs on as the
 * fly-out's title band, as wide as the fly-out, and the body hangs from it
 * (or rises above it when it would leave the screen). */
function RowExtension(props: {
  top: number;
  left: number;
  width: number;
  design: Design;
  title: string;
  value?: string;
  bodyHeight: number;
  children: JSX.Element;
  onClick?: () => void;
}) {
  const below = () => props.top + ROW + props.bodyHeight <= 480 - 8;
  return (
    <div
      class="absolute"
      style={{
        left: `${props.left}px`,
        width: `${props.width}px`,
        top: `${props.top}px`,
        height: `${ROW}px`,
      }}
    >
      <div
        class={`flex h-full items-center justify-between pr-[12px] pl-[10px] text-[19px] ${props.design.card.class}`}
        style={{ ...props.design.panel, ...props.design.row(true), color: props.design.iconInk(true) }}
        onClick={() => props.onClick?.()}
      >
        <span class="truncate">{props.title}</span>
        <span class="tabular-nums">{props.value}</span>
      </div>
      <div
        class="absolute inset-x-0 px-[12px] py-[10px]"
        style={{
          height: `${props.bodyHeight}px`,
          top: below() ? `${ROW}px` : `${-props.bodyHeight}px`,
          ...props.design.body,
        }}
      >
        {props.children}
      </div>
    </div>
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
  design: Design;
  left: number;
  /** Stacked and centered on `left`, for the narrow gap between compare columns. */
  stacked?: boolean;
}) {
  return (
    <div
      class={`absolute bottom-[12px] flex gap-[4px] ${props.stacked ? '-translate-x-1/2 flex-col items-stretch' : ''}`}
      style={{ left: `${props.left}px` }}
    >
      <For each={props.hints}>
        {(hint) => (
          <div
            class="flex h-[32px] items-center gap-[8px] px-[6px] whitespace-nowrap"
            style={props.design.legend.box}
            onClick={() => props.send({ key: hint.key })}
          >
            <span
              class="flex h-[22px] min-w-[22px] items-center justify-center px-[3px] text-[14px] leading-none font-semibold"
              style={props.design.legend.key}
            >
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

type Fly = 'none' | 'shift';
type Mode = 'live' | 'raw';

export interface EditorState {
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
}

function valueName(setting: Setting, recipe: Recipe): string {
  return setting.kind === 'choice'
    ? setting.choices[recipe[setting.id]].name
    : (STEP_NAMES[String(setting.steps[recipe[setting.id]])] ??
        fmt(setting.steps[recipe[setting.id]]));
}

export function Editor(props: { state: EditorState; send?: Send }) {
  const s = () => props.state;
  const send: Send = (action) => props.send?.(action);
  const design = () => DESIGN;
  const focused = () => SETTINGS[s().focus];
  const [scroll, setScroll] = createSignal(0);
  const rowTop = () => s().focus * ROW - scroll();
  const colors = palette(null);
  const compare = () => (s().comparing ? s().snapshot : undefined);

  const hints = () => {
    const list: { glyph: string; label: string; key: string }[] = [];
    if (compare()) list.push({ glyph: 'AEL', label: 'A/B', key: 'Tab' });
    else if (focused().id === 'wb' && s().fly === 'none')
      list.push({ glyph: '▲', label: 'SHIFT', key: 's' });
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
        '--accent': s().mode === 'raw' ? 'var(--color-play)' : 'var(--m-accent)',
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
                <div
                  class={`absolute top-[10px] flex h-[30px] w-[30px] items-center justify-center text-[19px] ${LIGHT}`}
                  style={{
                    left: side === 'a' ? `${COMPARE_WIDTH + 8}px` : `${720 - COMPARE_WIDTH - 38}px`,
                    background: s().side === side ? 'var(--accent)' : 'rgb(0 0 0 / 0.6)',
                  }}
                  onClick={() => send({ focus: s().focus, side })}
                >
                  {side === 'a' ? 'A' : 'B'}
                </div>
              )}
            </For>
            <For each={['a', 'b'] as Side[]}>
              {(side) => (
                <Column
                  scroll={scroll()}
                  setScroll={setScroll}
                  recipe={side === 'a' ? a() : s().recipe}
                  other={side === 'a' ? s().recipe : a()}
                  focus={s().focus}
                  left={side === 'a' ? 0 : 720 - COMPARE_WIDTH}
                  width={COMPARE_WIDTH}
                  design={design()}
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
          scroll={scroll()}
          setScroll={setScroll}
          recipe={s().recipe}
          other={s().snapshot}
          focus={s().focus}
          left={0}
          width={WIDTH}
          design={design()}
          send={send}
        />
        <Show when={s().fly === 'none'}>
          <div
            class="absolute top-[10px] flex flex-col items-start gap-[4px]"
            style={{ left: `${WIDTH + 12}px` }}
          >
            <Show when={focused().id !== 'film'}>
              <span
                class={`px-[12px] py-[6px] text-[22px] ${design().card.class}`}
                style={design().card.style}
              >
                {design().text?.(focused().name) ?? focused().name}
              </span>
            </Show>
            <span
              class={`px-[12px] py-[4px] text-[22px] ${design().card.class}`}
              style={design().card.style}
            >
              {design().text?.(valueName(focused(), s().recipe)) ??
                valueName(focused(), s().recipe)}
            </span>
            <Show when={focused().id === 'film' ? FILMS[s().recipe.film].note : undefined}>
              {(note) => (
                <p
                  class="max-w-[400px] px-[12px] py-[8px] text-[19px] leading-[26px] font-light tracking-[0.03em]"
                  style={{ ...design().card.style }}
                >
                  {note()}
                </p>
              )}
            </Show>
          </div>
        </Show>
        <Show when={s().mode === 'raw'}>
          <span class="absolute top-[10px] right-[12px] bg-(--accent) px-[8px] py-[2px] text-[17px] font-bold">
            RAW
          </span>
        </Show>
        <Show when={s().fly === 'shift'}>
          <RowExtension
            top={rowTop()}
            left={WIDTH}
            width={270}
            design={design()}
            title="WB SHIFT"
            value={`R${fmt(s().recipe.shiftR)} B${fmt(s().recipe.shiftB)}`}
            bodyHeight={274}
            onClick={() => send({ key: 'Escape' })}
          >
            <div class="flex h-full items-center justify-center">
              <ShiftGrid r={s().recipe.shiftR} b={s().recipe.shiftB} send={send} />
            </div>
          </RowExtension>
        </Show>
      </Show>

      <Show when={s().fly !== 'shift'}>
        <Legend
          hints={hints()}
          send={send}
          design={design()}
          left={compare() ? 360 : WIDTH + 12}
          stacked={compare() !== undefined}
        />
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
  const count = setting.kind === 'slider' ? setting.steps.length : setting.choices.length;
  return edit(state, (r) => ({
    ...r,
    [setting.id]: Math.max(0, Math.min(count - 1, r[setting.id] + delta)),
  }));
}

export function onKey(state: EditorState, key: string): EditorState {
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
  if (key === 'a') return { ...state, snapshot: { ...state.recipe }, comparing: false, side: 'b' };
  if (key === 'q' && state.snapshot)
    return { ...state, comparing: !state.comparing, fly: 'none', side: 'b' };
  if (key === 'Tab' && state.comparing) return { ...state, side: state.side === 'a' ? 'b' : 'a' };
  if (key === 'Escape') return { ...state, fly: 'none', comparing: false };
  return state;
}

export function apply(state: EditorState, action: Action): EditorState {
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

export const INITIAL: EditorState = {
  mode: 'live',
  recipe: BASE,
  focus: 1,
  fly: 'none',
  comparing: false,
  side: 'b',
};

const EDITED: Recipe = {
  ...BASE,
  film: 4,
  shadow: at(LEVELS, 2),
  highlight: at(LEVELS, 1),
  color: at(LEVELS, -1),
  nr: at(NR_LEVELS, 'SNR'),
  grain: 2,
  grainSize: 1,
  clarity: at(CLARITY, 2),
  shiftR: 4,
  shiftB: -5,
};

/** Each design with the WB row focused and changed from A, beside other
 * changed and unchanged rows. */
const DESIGN_STATES = [
  {
    id: 'edited',
    title: 'Edited, with A kept',
    state: { ...INITIAL, snapshot: BASE, recipe: { ...EDITED, wb: 1 }, focus: 1 } satisfies EditorState,
  },
  {
    id: 'compare-ab',
    title: 'Compare A and B',
    state: { ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true } satisfies EditorState,
  },
];

const STATES: { id: string; title: string; state: EditorState }[] = [
  ...DESIGN_STATES,
  { id: 'slider', title: 'Slider row', state: { ...INITIAL, focus: 4 } },
  {
    id: 'nr',
    title: 'NOISE REDUCTION with the levels below -2',
    state: { ...INITIAL, focus: 9, recipe: EDITED },
  },
  { id: 'film-notes', title: 'Film simulation', state: { ...INITIAL, focus: 0 } },
  { id: 'grain-size', title: 'GRAIN SIZE under the grain icon', state: { ...INITIAL, focus: 13, recipe: EDITED } },
  { id: 'wb-shift', title: 'WB SHIFT', state: { ...INITIAL, focus: 1, fly: 'shift' } },
  {
    id: 'compare-b',
    title: 'Compare, editing B',
    state: { ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true },
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    state: { ...INITIAL, mode: 'raw', recipe: EDITED, focus: 5 },
  },
];

/** The custom settings editor's review states, one screen each. */
export function RecipeStates() {
  return (
    <>
      <h2 class="mt-[32px] mb-[10px] text-[17px] font-semibold">Custom settings editor</h2>
      <For each={STATES}>
        {(entry) => (
          <section id={entry.id} class="mt-[28px]">
            <h2 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h2>
            <Fit>
              <Editor state={entry.state} />
            </Fit>
          </section>
        )}
      </For>
    </>
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
