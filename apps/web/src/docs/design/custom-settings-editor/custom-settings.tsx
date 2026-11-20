import PHOTO from '../../../engine/assets/default-scene.png';
import { For, Show, createEffect, createSignal, onSettled, untrack } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Choice, Dial, type ChoiceItem } from '../../../design/controls';
import { COLUMN, SMALL, TRACK } from '../../../design/materials';
import { Artwork, Wordmark, modArtwork, stockArtwork } from '../../../design/artwork';
import { Button, Scroller, reveal } from '../../../design/column';
import { PickerList, pickerStep } from '../../../design/picker';
import { Card, Fit, KeyHint } from '../../../design/screen';
import '../../../design/design.css';

/* A recipe editor over live view: settings in a translucent left column,
   the picture in the right. Choice rows scroll their choices as stock icons,
   slider rows step through segments as FUJIMOD SET-UP's color bars do, and
   the WB SHIFT fly-out continues the focused row across the
   picture. Compare shows the column twice, A and B, scrolled together. The
   same screen serves RAW CONVERSION in playback.

   Stock icons come from `mise run stock-icons-svg -- --firmware x70-110`,
   served from the local review directory; they are never published. */


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

/** The menu pages' row height. */
const ROW = 57;
/** The icon column, before the stroke that starts each row's drawer. */
const ICON_COLUMN = 58;
const WIDTH = 280;
const COMPARE_WIDTH = 300;



/* ---- Input ------------------------------------------------------------------- */

type Side = 'a' | 'b';
type Action =
  | { key: string }
  | { focus: number; side?: Side }
  | { set: string; value: number; focus: number; side?: Side }
  | { shift: [number, number] }
  /** A white balance chosen and its WB SHIFT opened, from the list. */
  | { shiftFor: number };
export type Send = (action: Action) => void;

/* ---- Rows -------------------------------------------------------------------- */

/** Words for row icons where the artwork is not published. */
const ICON_WORDS: Record<string, string> = {
  [0x5f3]: 'Film',
  [0x609]: 'WB',
  [0x5f1]: 'DR',
  [0x5f6]: 'H',
  [0x607]: 'S',
  [0x5f0]: 'Color',
  [0x605]: 'Sharp',
  [0x603]: 'NR',
  d_range_priority: 'D-Pri',
  clarity: 'Clarity',
  film_grain: 'Grain',
};

function RowIconView(props: { icon: RowIcon; color: string }) {
  const i = () => props.icon;
  const word = (key: string | number, badge?: string) => <Wordmark word={ICON_WORDS[key] ?? String(key)} badge={badge} color={props.color} />;
  return (
    <Show
      when={'word' in i() ? undefined : i()}
      fallback={'word' in i() ? <Wordmark word={(i() as { word: string }).word} badge={(i() as { badge?: string }).badge} color={props.color} /> : undefined}
    >
      {(icon) => {
        const it = icon() as { stock: number } | { mod: string };
        // Q icons are 80 by 48 with their ink in the middle 52 by 40; at 0.75
        // the ink is the root menu's 39 by 30.
        return 'stock' in it ? (
          <Artwork src={stockArtwork(it.stock)} class="-mx-[10px] h-[36px] w-[60px]" color={props.color} fallback={word(it.stock)} />
        ) : (
          <Artwork src={modArtwork(it.mod)} class="-mx-[2px] h-[30px] w-[44px]" color={props.color} fallback={word(it.mod)} />
        );
      }}
    </Show>
  );
}

/** Row cards sit 3 pixels inside the row on each side and short of the column's edge. */
const INSET = { top: 3, bottom: 3, right: 6 };

/** One setting's controls, beside its group's icon. */
function SettingRow(props: {
  setting: Setting;
  index: number;
  recipe: Recipe;
  focused: boolean;
  changed: boolean;
  width: number;
  side?: Side;
  send: Send;
}) {
  const controlWidth = () => props.width - ICON_COLUMN - INSET.right;
  const controlHeight = ROW - INSET.top - INSET.bottom;
  const pick = (value: number) => props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  const focus = () => props.send({ focus: props.index, side: props.side });
  /* A row's choices never change: built once, so its strip keeps its cells. */
  const items: ChoiceItem[] =
    props.setting.kind === 'choice'
      ? props.setting.choices.map((choice): ChoiceItem => {
          const src = choice.icon === undefined ? undefined : stockArtwork(choice.icon);
          return src === undefined ? choice.name : { name: choice.name, art: (ink) => <Artwork src={src} class="h-[24px] w-[40px]" fillInk={props.setting.id === 'wb'} color={ink} /> };
        })
      : [];
  return (
    <div
      class="relative flex items-stretch"
      style={{ height: `${ROW}px`, 'padding-top': `${INSET.top}px`, 'padding-bottom': `${INSET.bottom}px`, 'padding-right': `${INSET.right}px` }}
    >
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <Choice
            items={items}
            value={props.recipe[props.setting.id]}
            width={controlWidth()}
            height={controlHeight}
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
            height={controlHeight}
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
 * icon brightens when any of them has the focus. */
function SettingGroup(props: { rows: number[]; recipe: Recipe; other?: Recipe; focus?: number; width: number; side?: Side; send: Send }) {
  const at = () => props.rows.indexOf(props.focus ?? -1);
  return (
    <div class="relative flex">
      <div
        class="flex shrink-0 items-center justify-center"
        style={{ width: `${ICON_COLUMN}px`, height: `${props.rows.length * ROW}px` }}
        onClick={() => props.send({ focus: props.rows[Math.max(0, at())], side: props.side })}
      >
        <RowIconView icon={SETTINGS[props.rows[0]].icon} color={at() >= 0 ? '#fff' : 'rgb(255 255 255 / 0.7)'} />
      </div>
      <div class="flex min-w-0 flex-1 flex-col">
        <For each={props.rows}>
          {(index) => (
            <SettingRow
              setting={SETTINGS[index]}
              index={index}
              recipe={props.recipe}
              focused={index === props.focus}
              changed={props.other !== undefined && props.other[SETTINGS[index].id] !== props.recipe[SETTINGS[index].id]}
              width={props.width}
              side={props.side}
              send={props.send}
            />
          )}
        </For>
      </div>
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
  side?: Side;
  active?: boolean;
  /** Shared by A and B so they scroll together, and read by the fly-outs. */
  scroll: number;
  saved?: boolean;
  raw?: boolean;
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
  /* A key moved the focus: bring its row into view. */
  let opened = false;
  createEffect(
    () => props.focus,
    (focus) => {
      if (!scroller || props.active === false) return;
      reveal(scroller, focus * ROW, (focus + 1) * ROW, ROW, opened, { top: untrack(() => props.scroll), height: 480 });
      opened = true;
    },
  );
  return (
    <Scroller
      ref={(element) => (scroller = element)}
      class="absolute inset-y-0"
      style={{ left: `${props.left}px`, width: `${props.width}px`, 'box-shadow': 'var(--m-column-edge)' }}
      onScroll={props.setScroll}
    >
      <For each={GROUPS}>
        {(rows) => (
          <SettingGroup
            rows={rows}
            recipe={props.recipe}
            other={props.other}
            focus={props.active !== false ? props.focus : undefined}
            width={props.width}
            side={props.side}
            send={props.send}
          />
        )}
      </For>
      <Show when={props.side === undefined}>
        <div class="px-[6px] pt-[10px] pb-[12px]">
          <Button label={props.saved ? 'SAVED' : props.raw ? 'CREATE' : 'SAVE'} class="w-full" focused={false} accent={props.saved} onClick={() => props.send({ key: 'save' })} />
        </div>
      </Show>
    </Scroller>
  );
}

/* ---- Fly-outs ---------------------------------------------------------------- */

/** The focused row continued past the column, in the column's material: a
 * band level with the row, its card the row card's height, naming the setting
 * and its value; the body hangs below the band, or rises above it when it
 * only fits there, and shrinks to the roomier side when it fits on neither. */
function RowExtension(props: {
  top: number;
  left: number;
  width: number;
  title: string;
  value?: string;
  bodyHeight: number;
  children: JSX.Element;
  onClick?: () => void;
}) {
  const MARGIN = 8;
  const roomBelow = () => 480 - MARGIN - props.top - ROW;
  const roomAbove = () => props.top - MARGIN;
  const below = () => roomBelow() >= props.bodyHeight || (roomAbove() < props.bodyHeight && roomBelow() >= roomAbove());
  const body = () => Math.min(props.bodyHeight, below() ? roomBelow() : roomAbove());
  return (
    <div
      class={`absolute flex overflow-hidden ${below() ? 'flex-col' : 'flex-col-reverse'}`}
      style={{
        ...COLUMN,
        left: `${props.left}px`,
        width: `${props.width}px`,
        top: `${below() ? props.top : props.top - body()}px`,
        height: `${ROW + body()}px`,
        'border-radius': '0 16px 16px 0',
      }}
    >
      <div
        class="flex shrink-0 items-center justify-between px-[16px]"
        style={{ height: `${ROW}px`, ...(below() ? {} : { 'box-shadow': 'var(--m-divider)' }) }}
        onClick={() => props.onClick?.()}
      >
        <span class={`truncate font-semibold ${SMALL}`}>{props.title}</span>
        <span class={`tabular-nums ${SMALL}`}>{props.value}</span>
      </div>
      <div
        class="flex min-h-0 grow flex-col px-[6px]"
        style={{ height: `${body()}px`, ...(below() ? { 'box-shadow': 'var(--m-divider)', 'padding-top': '6px', 'padding-bottom': '6px' } : { 'padding-top': '6px' }) }}
      >
        {props.children}
      </div>
    </div>
  );
}

/** WB SHIFT's map, as the camera draws it: 19 by 19 colored dots (green to
 * the left, red to the right, blue at the top, yellow at the bottom, white in
 * the middle), white center axes, and a hollow white square at R and B. */
function ShiftGrid(props: { r: number; b: number; send: Send }) {
  const PITCH = 12;
  const size = PITCH * 19;
  const center = (v: number) => (v + 9) * PITCH + PITCH / 2;
  const mix = (a: number[], b: number[], t: number) => a.map((c, i) => Math.round(c + (b[i] - c) * t));
  const dot = (r: number, b: number) => {
    const across = mix([255, 255, 255], r < 0 ? [40, 220, 40] : [235, 40, 70], Math.abs(r) / 9);
    const [red, green, blue] = mix(across, b > 0 ? [40, 110, 255] : [240, 215, 0], Math.abs(b) / 12);
    return `rgb(${red} ${green} ${blue})`;
  };
  const steps = Array.from({ length: 19 }, (_, i) => i - 9);
  return (
    <svg
      width={size}
      height={size}
      class="block"
      onClick={(e) => {
        const box = e.currentTarget.getBoundingClientRect();
        const k = size / box.width;
        const r = Math.floor(((e.clientX - box.left) * k) / PITCH) - 9;
        const b = 9 - Math.floor(((e.clientY - box.top) * k) / PITCH);
        props.send({ shift: [Math.max(-9, Math.min(9, r)), Math.max(-9, Math.min(9, b))] });
      }}
    >
      <For each={steps}>
        {(b) => <For each={steps}>{(r) => <rect x={center(r) - 1.5} y={center(-b) - 1.5} width={3} height={3} fill={dot(r, b)} />}</For>}
      </For>
      <line x1={size / 2} x2={size / 2} y1={0} y2={size} stroke="rgb(255 255 255 / 0.85)" stroke-width={1.5} />
      <line y1={size / 2} y2={size / 2} x1={0} x2={size} stroke="rgb(255 255 255 / 0.85)" stroke-width={1.5} />
      <rect x={center(props.r) - 6} y={center(-props.b) - 6} width={12} height={12} fill="none" stroke="#fff" stroke-width={2} />
    </svg>
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
  /** The cursor in the focused choice row's list, while it is open. */
  list?: number;
  /** Saved since the last change. */
  saved?: boolean;
  /** WB SHIFT's R and B as it opened, for BACK. */
  shiftFrom?: [number, number];
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
  const focused = () => SETTINGS[s().focus];
  const [scroll, setScroll] = createSignal(0);
  const rowTop = () => s().focus * ROW - scroll();
  const compare = () => (s().comparing ? s().snapshot : undefined);

  const hints = () => {
    const list: { glyph: string; label: string; key: string }[] = [];
    if (compare()) list.push({ glyph: 'AEL', label: 'A/B', key: 'Tab' });
    if (s().snapshot) list.push({ glyph: 'Q', label: s().comparing ? 'HIDE A' : 'A|B', key: 'q' });
    else list.push({ glyph: 'AF-ON', label: 'KEEP A', key: 'a' });
    list.push({ glyph: 'OK', label: focused().kind === 'choice' && !compare() ? 'SELECT' : s().mode === 'raw' ? 'CREATE' : 'SAVE', key: 'Enter' });
    return list;
  };

  return (
    <div
      class="absolute inset-0"
      style={{ '--m-accent': s().mode === 'raw' ? 'var(--color-play)' : undefined }}
    >
      <Photo recipe={s().recipe} />
      <Show when={compare()}>
        {(a) => (
          <>
            <div class="absolute inset-y-0 left-0 w-[360px] overflow-hidden">
              <div class="absolute inset-y-0 left-0 w-[720px]">
                <Photo recipe={a()} />
              </div>
            </div>
            <div class="absolute inset-y-0 left-[359px] w-[2px] bg-white" />
            <For each={['a', 'b'] as Side[]}>
              {(side) => (
                <div class="absolute top-[10px]" style={side === 'a' ? { left: `${COMPARE_WIDTH + 8}px` } : { right: `${COMPARE_WIDTH + 8}px` }}>
                  <KeyHint glyph={side === 'a' ? 'A' : 'B'} accent={s().side === side} tap={() => send({ focus: s().focus, side })} />
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
        <Column scroll={scroll()} setScroll={setScroll} recipe={s().recipe} other={s().snapshot} focus={s().focus} left={0} width={WIDTH} saved={s().saved} raw={s().mode === 'raw'} send={send} />
        <Show when={s().list !== undefined && focused().kind === 'choice' ? (focused() as ChoiceSetting) : undefined}>
          {(setting) => (
            <RowExtension top={rowTop()} left={WIDTH} width={720 - WIDTH - 8} title={setting().name} value={valueName(setting(), s().recipe)} bodyHeight={setting().choices.length * 44 + 12} onClick={() => send({ key: 'Escape' })}>
            <PickerList
              options={setting().choices.map((choice) => {
                const wb = setting().id === 'wb';
                const src = choice.icon === undefined ? undefined : stockArtwork(choice.icon);
                return {
                  label: choice.name,
                  art: src === undefined ? undefined : (ink: string) => <Artwork src={src} class="h-[30px] w-[50px]" fillInk={wb} color={ink} />,
                  more: setting().id === 'wb' ? () => send({ shiftFor: setting().choices.indexOf(choice) }) : undefined,
                };
              })}
              at={s().list ?? 0}
              current={s().recipe[setting().id]}
              choose={(value) => send({ set: setting().id, value, focus: s().focus })}
            />
            </RowExtension>
          )}
        </Show>
        <Show when={s().fly === 'none' && s().list === undefined}>
          <div class="absolute top-[10px] flex flex-col items-start gap-[4px]" style={{ left: `${WIDTH + 12}px` }}>
            <Show when={focused().id !== 'film'}>
              <Card class={`px-[12px] py-[6px] ${SMALL}`}>{focused().name}</Card>
            </Show>
            <Card class={`px-[12px] py-[6px] ${SMALL}`}>{valueName(focused(), s().recipe)}</Card>
            <Show when={focused().id === 'film' ? FILMS[s().recipe.film].note : undefined}>
              {(note) => <Card class="max-w-[400px] px-[12px] py-[8px] text-[19px] leading-[26px] font-light">{note()}</Card>}
            </Show>
          </div>
        </Show>
        <Show when={s().mode === 'raw'}>
          <div class="absolute top-[10px] right-[10px]">
            <KeyHint glyph="RAW" accent />
          </div>
        </Show>
        <Show when={s().fly === 'shift'}>
          <RowExtension
            top={rowTop()}
            left={WIDTH}
            width={270}
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

      <Show when={s().fly !== 'shift' && s().list === undefined}>
        <div
          class={`absolute bottom-[12px] flex gap-[4px] ${compare() ? '-translate-x-1/2 flex-col items-stretch' : ''}`}
          style={{ left: `${compare() ? 360 : WIDTH + 12}px` }}
        >
          <For each={hints()}>{(hint) => <KeyHint glyph={hint.glyph} label={hint.label} tap={() => send({ key: hint.key })} />}</For>
        </div>
      </Show>
    </div>
  );
}

/* ---- The live screen and gallery --------------------------------------------- */

/** Applies an edit to the column that has focus. */
function edit(state: EditorState, change: (r: Recipe) => Recipe): EditorState {
  if (state.comparing && state.side === 'a' && state.snapshot)
    return { ...state, snapshot: change(state.snapshot), saved: false };
  return { ...state, recipe: change(state.recipe), saved: false };
}

function step(state: EditorState, delta: number): EditorState {
  const setting = SETTINGS[state.focus];
  const count = setting.kind === 'slider' ? setting.steps.length : setting.choices.length;
  return edit(state, (r) => ({
    ...r,
    [setting.id]: Math.max(0, Math.min(count - 1, r[setting.id] + delta)),
  }));
}

export function onKey(state: EditorState, input: string): EditorState {
  const setting = SETTINGS[state.focus];
  const dial = input === 'DialLeft' ? -1 : input === 'DialRight' ? 1 : 0;
  /* An option list, as stock's: UP, DOWN and the dial move; MENU/OK and LEFT
     choose; RIGHT on a white balance opens its WB SHIFT; BACK closes. */
  if (state.list !== undefined && setting.kind === 'choice') {
    const keyName = ({ ArrowUp: 'up', ArrowDown: 'down', DialLeft: 'dial-', DialRight: 'dial+' } as Record<string, string>)[input];
    const at = keyName === undefined ? undefined : pickerStep(state.list, setting.choices.length, keyName);
    if (at !== undefined) return { ...state, list: at };
    const chosen = state.list;
    const picked = { ...edit(state, (r) => ({ ...r, [setting.id]: chosen })), list: undefined };
    if (input === 'Enter' || input === 'ArrowLeft') return picked;
    if (input === 'ArrowRight' && setting.id === 'wb') return openShift(picked);
    if (input === 'Escape') return { ...state, list: undefined };
    return state;
  }
  /* WB SHIFT, as stock's grid: the arrows move the point, the dial does
     nothing, MENU/OK keeps the shift, BACK puts back what it opened with. */
  if (state.fly === 'shift') {
    const clamp = (n: number) => Math.max(-9, Math.min(9, n));
    if (input === 'ArrowLeft') return edit(state, (r) => ({ ...r, shiftR: clamp(r.shiftR - 1) }));
    if (input === 'ArrowRight') return edit(state, (r) => ({ ...r, shiftR: clamp(r.shiftR + 1) }));
    if (input === 'ArrowUp') return edit(state, (r) => ({ ...r, shiftB: clamp(r.shiftB + 1) }));
    if (input === 'ArrowDown') return edit(state, (r) => ({ ...r, shiftB: clamp(r.shiftB - 1) }));
    if (input === 'Enter') return { ...state, fly: 'none', shiftFrom: undefined };
    if (input === 'Escape') {
      const [shiftR, shiftB] = state.shiftFrom ?? [state.recipe.shiftR, state.recipe.shiftB];
      return { ...edit(state, (r) => ({ ...r, shiftR, shiftB })), fly: 'none', shiftFrom: undefined };
    }
    return state;
  }
  const key = dial < 0 ? 'ArrowLeft' : dial > 0 ? 'ArrowRight' : input;
  if (key === 'Fn' && setting.id === 'wb' && state.fly === 'none') return openShift(state);
  if (key === 'Enter' && setting.kind === 'choice' && !state.comparing) return { ...state, list: state.recipe[setting.id] };
  if (key === 'save' || key === 'Enter') return { ...state, saved: true };
  if (key === 'ArrowUp' || key === 'ArrowDown') {
    const focus = Math.max(0, Math.min(SETTINGS.length - 1, state.focus + (key === 'ArrowUp' ? -1 : 1)));
    return { ...state, focus, fly: 'none' };
  }
  if (key === 'ArrowLeft') return step(state, -1);
  if (key === 'ArrowRight') return step(state, 1);
  if (key === 'a') return { ...state, snapshot: { ...state.recipe }, comparing: false, side: 'b' };
  if (key === 'q' && state.snapshot) return { ...state, comparing: !state.comparing, fly: 'none', side: 'b' };
  if (key === 'Tab' && state.comparing) return { ...state, side: state.side === 'a' ? 'b' : 'a' };
  if (key === 'Escape') return { ...state, fly: 'none', comparing: false };
  return state;
}

function openShift(state: EditorState): EditorState {
  const r = state.comparing && state.side === 'a' && state.snapshot ? state.snapshot : state.recipe;
  return { ...state, fly: 'shift', shiftFrom: [r.shiftR, r.shiftB] };
}

export function apply(state: EditorState, action: Action): EditorState {
  if ('key' in action) return onKey(state, action.key);
  if ('shiftFor' in action) return openShift({ ...edit(state, (r) => ({ ...r, wb: action.shiftFor })), list: undefined });
  if ('shift' in action)
    return edit(state, (r) => ({ ...r, shiftR: action.shift[0], shiftB: action.shift[1] }));
  const moved = {
    ...state,
    focus: action.focus,
    side: action.side ?? state.side,
    fly: action.focus === state.focus ? state.fly : ('none' as Fly),
    list: undefined,
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

export const SETTINGS_STATES: { id: string; title: string; state: EditorState }[] = [
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
  { id: 'slider', title: 'Slider row', state: { ...INITIAL, focus: 4 } },
  {
    id: 'nr',
    title: 'NOISE REDUCTION with the levels below -2',
    state: { ...INITIAL, focus: 9, recipe: EDITED },
  },
  { id: 'film-notes', title: 'Film simulation', state: { ...INITIAL, focus: 0 } },
  { id: 'film-list', title: 'FILM SIMULATION list, opened with OK', state: { ...INITIAL, focus: 0, list: 3 } },
  { id: 'wb-list', title: 'WHITE BALANCE list', state: { ...INITIAL, focus: 1, list: 2 } },
  { id: 'grain-size', title: 'GRAIN SIZE under the grain icon', state: { ...INITIAL, focus: 13, recipe: EDITED } },
  { id: 'wb-shift', title: 'WB SHIFT, opened with RIGHT from the WHITE BALANCE list', state: { ...INITIAL, focus: 1, fly: 'shift', shiftFrom: [0, 0] } },
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
