import { render } from '@solidjs/web';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
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
  steps: Step[];
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
    steps: LEVELS,
  },
  { kind: 'slider', id: 'shadow', name: 'SHADOW TONE', icon: { stock: 0x607 }, steps: LEVELS },
  { kind: 'slider', id: 'color', name: 'COLOR', icon: { stock: 0x5f0 }, steps: LEVELS },
  { kind: 'slider', id: 'sharp', name: 'SHARPNESS', icon: { stock: 0x605 }, steps: LEVELS },
  { kind: 'slider', id: 'nr', name: 'NOISE REDUCTION', icon: { stock: 0x603 }, steps: NR_LEVELS },
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

/** Every setting holds the index of its step or choice; WB SHIFT holds R and B. */
type Recipe = Record<string, number> & { shiftR: number; shiftB: number };

const at = (steps: Step[], value: Step) => steps.indexOf(value);
const BASE: Recipe = {
  film: 3,
  wb: 0,
  dr: 0,
  highlight: at(LEVELS, -1),
  shadow: at(LEVELS, 1),
  color: at(LEVELS, 2),
  sharp: at(LEVELS, -1),
  nr: at(NR_LEVELS, -2),
  chrome: 1,
  fxblue: 0,
  grain: 1,
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

/* Five whole designs of the same layout, beside the embossed one built first.
   Each sets every surface: the column, its rows and focus, the icon column,
   the chosen choice, the slider's form, the title box, the legend and the
   fly-outs. */

type DesignId = 'embossed' | 'paper' | 'viewfinder' | 'dial' | 'meter' | 'outline';

interface Mark {
  focused: boolean;
  changed: boolean;
}

/** Corner brackets, as an AF frame draws them. */
interface CornerMarks {
  arm: number;
  weight: number;
  color: string;
  /** The lower brackets' color, when it differs. */
  lower?: string;
}

/** A chosen cell or a thumb's box: its own surface and edges, its ink, and marks. */
interface Look {
  style: JSX.CSSProperties;
  ink: string;
  corners?: CornerMarks;
  /** A small square in the top right corner. */
  pip?: string;
}

/** How a slider row draws its steps. */
type SliderForm =
  /** A track with ticks, zero to the value filled, a square thumb carrying the number. */
  | 'thumb'
  /** A scale of ticks with a full-height needle on the value, the number at the end. */
  | 'ruler'
  /** Every step printed; the value large, the others small. */
  | 'digits'
  /** One block per step, zero to the value lit, the number at the end. */
  | 'blocks';

interface SliderColors {
  track: string;
  tick: string;
  zero: string;
  fill: string;
  /** The value's box (thumb, needle label or lit digit) and its ink. */
  value: (mark: Mark) => Look;
  /** Steps printed but not chosen ('digits'). */
  other: string;
}

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
  lines: { top: number; bottom: number };
  /** How the focused row is marked, beyond its own surface. */
  focus: 'stripe' | 'corners' | 'frame' | 'none';
  focusColor: string;
  iconInk: (focused: boolean) => string;
  /** The stroke after the icon column. */
  divider?: (focused: boolean) => string | undefined;
  drawer?: (focused: boolean) => string | undefined;
  choiceFont: string;
  chosen: (mark: Mark) => Look;
  /** Ink of the choices not chosen. */
  other: (focused: boolean) => string;
  slider: SliderForm;
  sliderColors: (focused: boolean) => SliderColors;
  card: Card;
  legend: { box: JSX.CSSProperties; key: JSX.CSSProperties };
  /** The fly-out's body below its band; the band is the focused row. */
  body: JSX.CSSProperties;
  /** Drawn under everything in the column, for legibility over the picture. */
  filter?: string;
}

const ACCENT = 'var(--accent)';
const ring = (width: number, color: string) => `inset 0 0 0 ${width}px ${color}`;
const top = (width: number, color: string) => `inset 0 ${width}px 0 ${color}`;
const bottom = (width: number, color: string) => `inset 0 -${width}px 0 ${color}`;
/** Box shadows listed first paint on top; false entries drop out. */
const edges = (...list: (string | false | undefined)[]) =>
  list.filter((edge): edge is string => typeof edge === 'string').join(', ') || undefined;

const PAPER = 'rgb(236 234 228 / 0.95)';
const PAPER_INK = '#16161a';
const DIAL_PLATE = 'rgb(46 46 48 / 0.97)';
const DIAL_LIT = '#f1f1ef';

const DESIGNS: Record<DesignId, Design> = {
  embossed: {
    label: 'Embossed',
    panel: { background: 'rgb(34 34 36 / 0.88)' },
    edge: (side) =>
      side === 'right'
        ? 'inset -1px 0 0 rgb(255 255 255 / 0.3), inset -3px 0 0 #000'
        : 'inset 1px 0 0 rgb(255 255 255 / 0.3), inset 3px 0 0 #000',
    row: (focused) => ({
      background: focused ? 'rgb(88 88 92 / 0.96)' : undefined,
      'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.2), inset 0 -2px 0 #000',
    }),
    lines: { top: 1, bottom: 2 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    divider: () => 'rgb(255 255 255 / 0.22)',
    drawer: () => 'rgb(255 255 255 / 0.11)',
    choiceFont: 'text-[16px] font-light tracking-[0.08em] uppercase',
    chosen: (m) => ({
      style: {
        background: 'rgb(255 255 255 / 0.2)',
        'box-shadow': edges(m.changed && bottom(3, ACCENT), m.focused && ring(3, '#fff')),
      },
      ink: '#fff',
    }),
    other: () => '#fff',
    slider: 'thumb',
    sliderColors: () => ({
      track: 'rgb(255 255 255 / 0.35)',
      tick: 'rgb(255 255 255 / 0.55)',
      zero: '#fff',
      fill: ACCENT,
      value: (m) => ({
        style: { background: '#fff', 'box-shadow': m.focused ? ring(3, '#fff') : undefined },
        ink: m.changed ? ACCENT : '#000',
      }),
      other: '#fff',
    }),
    card: {
      style: { background: 'rgb(24 24 24 / 0.82)', color: '#fff' },
      class: 'font-light tracking-[0.14em] uppercase',
    },
    legend: {
      box: { background: 'rgb(0 0 0 / 0.6)', color: '#fff' },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(30 30 32 / 0.96)', 'box-shadow': ring(1, '#3e3e42'), color: '#fff' },
  },

  /* Printed, like the engraving on a top dial: black ink on warm paper, the
     chosen choice a solid black block, a change in red. */
  paper: {
    label: 'Paper',
    panel: { background: PAPER },
    edge: (side) => (side === 'right' ? 'inset -2px 0 0 #000' : 'inset 2px 0 0 #000'),
    row: (focused) => ({
      background: focused ? 'rgb(0 0 0 / 0.08)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(0 0 0 / 0.16)',
    }),
    lines: { top: 0, bottom: 1 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => PAPER_INK,
    divider: () => 'rgb(0 0 0 / 0.2)',
    choiceFont: 'text-[16px] font-medium tracking-[0.06em] uppercase',
    chosen: (m) => ({
      style: {
        background: m.focused ? PAPER_INK : 'rgb(0 0 0 / 0.14)',
        'box-shadow': m.changed ? bottom(4, ACCENT) : undefined,
      },
      ink: m.focused ? '#fff' : PAPER_INK,
    }),
    other: () => 'rgb(22 22 26 / 0.75)',
    slider: 'thumb',
    sliderColors: () => ({
      track: 'rgb(0 0 0 / 0.3)',
      tick: 'rgb(0 0 0 / 0.45)',
      zero: PAPER_INK,
      fill: ACCENT,
      value: (m) => ({
        style: {
          background: m.focused ? PAPER_INK : PAPER,
          'box-shadow': edges(m.changed && bottom(4, ACCENT), ring(2, PAPER_INK)),
        },
        ink: m.focused ? '#fff' : PAPER_INK,
      }),
      other: PAPER_INK,
    }),
    card: {
      style: { background: PAPER, color: PAPER_INK, 'box-shadow': ring(2, PAPER_INK) },
      class: 'font-medium tracking-[0.12em] uppercase',
    },
    legend: {
      box: { background: PAPER, color: PAPER_INK },
      key: { background: PAPER_INK, color: '#fff' },
    },
    body: { background: PAPER, color: PAPER_INK, 'box-shadow': ring(2, PAPER_INK) },
  },

  /* The EVF's own language: no boxes, white type with a hard shadow straight
     on a dimmed picture, the focused row held in AF-frame corners, sliders as
     an exposure scale with a needle. */
  viewfinder: {
    label: 'Viewfinder',
    panel: { background: 'rgb(0 0 0 / 0.42)' },
    edge: () => undefined,
    row: (focused) => ({ background: focused ? 'rgb(0 0 0 / 0.38)' : undefined }),
    lines: { top: 0, bottom: 0 },
    focus: 'corners',
    focusColor: '#fff',
    iconInk: () => '#fff',
    choiceFont: 'text-[16px] font-normal tracking-[0.1em] uppercase',
    chosen: (m) => ({
      style: { 'box-shadow': bottom(3, m.changed ? ACCENT : '#fff') },
      ink: '#fff',
    }),
    other: (focused) => (focused ? 'rgb(255 255 255 / 0.62)' : 'rgb(255 255 255 / 0.45)'),
    slider: 'ruler',
    sliderColors: (focused) => ({
      track: 'rgb(255 255 255 / 0.6)',
      tick: 'rgb(255 255 255 / 0.7)',
      zero: '#fff',
      fill: focused ? '#fff' : 'rgb(255 255 255 / 0.85)',
      value: (m) => ({ style: {}, ink: m.changed ? '#ff5a4a' : '#fff' }),
      other: '#fff',
    }),
    card: {
      style: { color: '#fff', 'text-shadow': '0 2px 0 #000, 0 0 3px #000' },
      class: 'font-normal tracking-[0.14em] uppercase',
    },
    legend: {
      box: { background: 'rgb(0 0 0 / 0.45)', color: '#fff' },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(0 0 0 / 0.62)', color: '#fff' },
    filter: 'drop-shadow(0 1px 0 #000)',
  },

  /* Machined plates with gaps between them. The focused plate lights up white
     with black ink; only the chosen choice is bright, and sliders print every
     step like a dial's scale with the value large. */
  dial: {
    label: 'Dial',
    panel: { background: 'rgb(12 12 12 / 0.9)' },
    edge: () => undefined,
    row: (focused) => ({
      background: focused ? DIAL_LIT : DIAL_PLATE,
      'box-shadow': edges(bottom(3, 'rgb(12 12 12)'), focused && bottom(5, ACCENT)),
    }),
    lines: { top: 0, bottom: 3 },
    focus: 'none',
    focusColor: ACCENT,
    iconInk: (focused) => (focused ? '#000' : '#fff'),
    divider: (focused) => (focused ? 'rgb(0 0 0 / 0.25)' : 'rgb(0 0 0 / 0.6)'),
    choiceFont: 'text-[17px] font-semibold tracking-[0.04em] uppercase',
    chosen: (m) => ({
      style: {},
      ink: m.changed ? ACCENT : m.focused ? '#000' : '#fff',
    }),
    other: (focused) => (focused ? 'rgb(0 0 0 / 0.28)' : 'rgb(255 255 255 / 0.26)'),
    slider: 'digits',
    sliderColors: (focused) => ({
      track: 'transparent',
      tick: 'transparent',
      zero: 'transparent',
      fill: ACCENT,
      value: (m) => ({ style: {}, ink: m.changed ? ACCENT : focused ? '#000' : '#fff' }),
      other: focused ? 'rgb(0 0 0 / 0.32)' : 'rgb(255 255 255 / 0.3)',
    }),
    card: {
      style: { background: DIAL_LIT, color: '#000' },
      class: 'font-semibold tracking-[0.06em] uppercase',
    },
    legend: {
      box: { background: DIAL_LIT, color: '#000' },
      key: { background: '#000', color: '#fff' },
    },
    body: { background: 'rgb(12 12 12 / 0.94)', color: '#fff', 'box-shadow': ring(2, DIAL_LIT) },
  },

  /* An instrument panel: near-black, the focused row held in a red frame,
     choices as indicator lamps (a lit bar over the chosen one), sliders as
     segment meters lit from zero. */
  meter: {
    label: 'Meter',
    panel: { background: 'rgb(6 6 6 / 0.84)' },
    edge: (side) => (side === 'right' ? 'inset -1px 0 0 #333' : 'inset 1px 0 0 #333'),
    row: () => ({ 'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.1)' }),
    lines: { top: 0, bottom: 1 },
    focus: 'frame',
    focusColor: ACCENT,
    iconInk: (focused) => (focused ? '#fff' : 'rgb(255 255 255 / 0.8)'),
    choiceFont: 'text-[15px] font-medium tracking-[0.12em] uppercase',
    chosen: (m) => ({
      style: {
        'box-shadow': top(4, m.focused ? ACCENT : 'rgb(255 255 255 / 0.85)'),
        background: 'rgb(255 255 255 / 0.06)',
      },
      ink: '#fff',
      pip: m.changed ? ACCENT : undefined,
    }),
    other: () => 'rgb(255 255 255 / 0.38)',
    slider: 'blocks',
    sliderColors: (focused) => ({
      track: 'rgb(255 255 255 / 0.12)',
      tick: 'rgb(255 255 255 / 0.12)',
      zero: 'rgb(255 255 255 / 0.5)',
      fill: focused ? ACCENT : 'rgb(255 255 255 / 0.82)',
      value: (m) => ({ style: {}, ink: m.changed ? '#ff5a4a' : '#fff' }),
      other: '#fff',
    }),
    card: {
      style: { background: 'rgb(6 6 6 / 0.84)', color: '#fff', 'box-shadow': top(3, ACCENT) },
      class: 'font-medium tracking-[0.14em] uppercase',
    },
    legend: {
      box: { background: 'rgb(6 6 6 / 0.84)', color: '#fff' },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(6 6 6 / 0.94)', color: '#fff', 'box-shadow': ring(2, ACCENT) },
  },

  /* Wireframe: hairlines only over a light veil, everything drawn in 1px
     white strokes; focus thickens and fills. */
  outline: {
    label: 'Outline',
    panel: { background: 'rgb(0 0 0 / 0.5)' },
    edge: (side) => (side === 'right' ? 'inset -1px 0 0 #fff' : 'inset 1px 0 0 #fff'),
    row: (focused) => ({
      background: focused ? 'rgb(255 255 255 / 0.1)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.4)',
    }),
    lines: { top: 0, bottom: 1 },
    focus: 'frame',
    focusColor: '#fff',
    iconInk: () => '#fff',
    divider: () => 'rgb(255 255 255 / 0.4)',
    choiceFont: 'text-[16px] font-light tracking-[0.1em] uppercase',
    chosen: (m) => ({
      style: {
        'box-shadow': ring(m.focused ? 2 : 1, m.changed ? ACCENT : '#fff'),
        background: m.focused ? 'rgb(255 255 255 / 0.14)' : undefined,
      },
      ink: '#fff',
    }),
    other: () => 'rgb(255 255 255 / 0.7)',
    slider: 'thumb',
    sliderColors: () => ({
      track: 'rgb(255 255 255 / 0.7)',
      tick: 'rgb(255 255 255 / 0.7)',
      zero: '#fff',
      fill: '#fff',
      value: (m) => ({
        style: {
          background: m.focused ? '#fff' : 'rgb(0 0 0 / 0.75)',
          'box-shadow': ring(m.focused ? 2 : 1, m.changed ? ACCENT : '#fff'),
        },
        ink: m.focused ? '#000' : '#fff',
      }),
      other: '#fff',
    }),
    card: {
      style: { background: 'rgb(0 0 0 / 0.5)', color: '#fff', 'box-shadow': ring(1, '#fff') },
      class: 'font-light tracking-[0.14em] uppercase',
    },
    legend: {
      box: { background: 'rgb(0 0 0 / 0.5)', color: '#fff', 'box-shadow': ring(1, '#fff') },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(0 0 0 / 0.72)', color: '#fff', 'box-shadow': ring(1, '#fff') },
  },
};

/* ---- Layout ------------------------------------------------------------------ */

/** Eleven rows fill the 480-pixel screen without scrolling. */
const ROW = 43;
const ICON_CELL = 52;
/** The icon column, before the stroke that starts each row's drawer. */
const ICON_COLUMN = 58;
const WIDTH = 280;
const COMPARE_WIDTH = 300;
const STRIPE = 4;
/** Where clipped rows or choices fade out, to show that more follow. */
const FADE = 18;
/** The focus frame around a value thumb, and the gap between the frame and the thumb. */
const FRAME = 3;
const FRAME_GAP = 2;
/** From a slider's ends to its first and last steps: half a framed value thumb. */
const INSET = 26;

/** The same scroll for every column, so A and B stay in lockstep. */
function scrollFor(focus: number, height: number): number {
  const max = Math.max(0, SETTINGS.length * ROW - height);
  return Math.max(0, Math.min(max, focus * ROW - (height - ROW) / 2));
}

const LIGHT = 'font-light tracking-[0.08em] uppercase';
/** FUJIMOD SET-UP's selection: a white outline just outside the cell. */
const SELECTED = 'outline-[3px] outline-offset-[2px] outline-white';

function Corners(props: CornerMarks) {
  const corners = [
    ['top', 'left'],
    ['top', 'right'],
    ['bottom', 'left'],
    ['bottom', 'right'],
  ] as const;
  const arm = (v: string, h: string, width: number, height: number): JSX.CSSProperties => ({
    [v]: '0',
    [h]: '0',
    width: `${width}px`,
    height: `${height}px`,
    background: v === 'bottom' ? (props.lower ?? props.color) : props.color,
  });
  return (
    <For each={corners}>
      {([v, h]) => (
        <>
          <span class="pointer-events-none absolute" style={arm(v, h, props.arm, props.weight)} />
          <span class="pointer-events-none absolute" style={arm(v, h, props.weight, props.arm)} />
        </>
      )}
    </For>
  );
}

/** A look's marks over its cell. */
function Marks(props: { look: Look }) {
  return (
    <>
      <Show when={props.look.corners}>{(corners) => <Corners {...corners()} />}</Show>
      <Show when={props.look.pip}>
        {(color) => (
          <span
            class="pointer-events-none absolute top-0 right-0 h-[6px] w-[6px]"
            style={{ background: color() }}
          />
        )}
      </Show>
    </>
  );
}

/* ---- Input ------------------------------------------------------------------- */

type Side = 'a' | 'b';
type Action =
  | { key: string }
  | { focus: number; side?: Side }
  | { set: string; value: number; focus: number; side?: Side }
  | { shift: [number, number] };
type Send = (action: Action) => void;

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

function RowIconView(props: { icon: RowIcon; color: string }) {
  return (
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
  );
}

interface ControlProps {
  design: Design;
  focused: boolean;
  changed: boolean;
  pick: (value: number) => void;
}

/** Every choice in a row, scrolled to keep the chosen one in view. */
function ChoiceStrip(props: ControlProps & { setting: ChoiceSetting; value: number; width: number }) {
  const look = () => props.design.chosen({ focused: props.focused, changed: props.changed });
  let track: HTMLDivElement | undefined;
  const [offset, setOffset] = createSignal(0);
  const [more, setMore] = createSignal(false);
  const follow = () => {
    const chosen = track?.children[props.value] as HTMLElement | undefined;
    if (!track || !chosen) return;
    // Start on a whole choice, the one that best centers the chosen one.
    const centered = chosen.offsetLeft + chosen.offsetWidth / 2 - props.width / 2;
    const end = chosen.offsetLeft + chosen.offsetWidth;
    let best = 0;
    const fits = track.scrollWidth <= props.width;
    for (const cell of Array.from(track.children) as HTMLElement[]) {
      const start = cell.offsetLeft;
      if (fits || start > chosen.offsetLeft || end - start > props.width) continue;
      if (Math.abs(start - centered) < Math.abs(best - centered)) best = start;
    }
    setOffset(best);
    setMore(track.scrollWidth - best > props.width + 1);
  };
  createEffect(
    () => [props.value, props.width],
    () => {
      requestAnimationFrame(follow);
    },
  );
  return (
    <div
      class="relative h-full overflow-hidden"
      style={{
        width: `${props.width}px`,
        'mask-image': `linear-gradient(to right, ${offset() > 0 ? 'transparent' : '#000'}, #000 ${FADE}px, #000 calc(100% - ${FADE}px), ${more() ? 'transparent' : '#000'})`,
      }}
    >
      <div
        ref={(element) => (track = element)}
        class="absolute inset-y-0 left-0 flex items-stretch"
        style={{ transform: `translateX(${-offset()}px)` }}
      >
        <For each={props.setting.choices.map((choice, i) => ({ choice, i }))}>
          {(item) => {
            const chosen = () => item.i === props.value;
            const ink = () => (chosen() ? look().ink : props.design.other(props.focused));
            return (
              <div
                class="relative flex shrink-0 items-center justify-center"
                style={{
                  width: item.choice.icon ? `${ICON_CELL}px` : undefined,
                  padding: item.choice.icon ? undefined : '0 10px',
                  ...(chosen() ? look().style : {}),
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  props.pick(item.i);
                }}
              >
                <Show
                  when={item.choice.icon}
                  fallback={
                    <span class={props.design.choiceFont} style={{ color: ink() }}>
                      {item.choice.name}
                    </span>
                  }
                >
                  {(icon) => <Ink src={stock(icon())} class="h-[24px] w-[48px]" color={ink()} />}
                </Show>
                <Show when={chosen()}>
                  <Marks look={look()} />
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

type SliderProps = ControlProps & { setting: SliderSetting; value: number };

/** Picks the step nearest a click across `inset` to `width - inset`. */
function pickStep(e: MouseEvent, count: number, inset: number, pick: (value: number) => void) {
  const element = e.currentTarget as HTMLElement;
  e.stopPropagation();
  const box = element.getBoundingClientRect();
  const scale = box.width / element.offsetWidth;
  const inner = (e.clientX - box.left) / scale - inset;
  const i = Math.round((inner / (element.offsetWidth - 2 * inset)) * (count - 1));
  pick(Math.max(0, Math.min(count - 1, i)));
}

function SliderView(props: SliderProps) {
  return (
    <>
      <Show when={props.design.slider === 'thumb'}>
        <ThumbSlider {...props} />
      </Show>
      <Show when={props.design.slider === 'ruler'}>
        <RulerSlider {...props} />
      </Show>
      <Show when={props.design.slider === 'digits'}>
        <DigitSlider {...props} />
      </Show>
      <Show when={props.design.slider === 'blocks'}>
        <BlockSlider {...props} />
      </Show>
    </>
  );
}

/** FUJIMOD SET-UP's segment bar: zero to the value filled, the value itself a
 * square thumb that carries the number. */
function ThumbSlider(props: SliderProps) {
  const colors = () => props.design.sliderColors(props.focused);
  const look = () => colors().value({ focused: props.focused, changed: props.changed });
  const count = () => props.setting.steps.length;
  const zero = () => props.setting.steps.indexOf(0);
  const x = (i: number) => `calc(${INSET}px + (100% - ${2 * INSET}px) * ${i / (count() - 1)})`;
  return (
    <div class="relative mx-[6px] flex-1" onClick={(e) => pickStep(e, count(), INSET, props.pick)}>
      <div
        class="absolute top-1/2 h-[2px] -translate-y-1/2"
        style={{ left: `${INSET}px`, right: `${INSET}px`, background: colors().track }}
      />
      <div
        class="absolute top-1/2 h-[4px] -translate-y-1/2"
        style={{
          left: x(Math.min(zero(), props.value)),
          width: `calc((100% - ${2 * INSET}px) * ${Math.abs(props.value - zero()) / (count() - 1)})`,
          background: colors().fill,
        }}
      />
      <For each={props.setting.steps.map((step, i) => i)}>
        {(i) => (
          <span
            class="absolute top-1/2 -translate-x-1/2 -translate-y-1/2"
            style={{
              left: x(i),
              width: i === zero() ? '4px' : '2px',
              height: i === zero() ? '26px' : '10px',
              background: i === zero() ? colors().zero : colors().tick,
            }}
          />
        )}
      </For>
      <span
        class="absolute inset-y-0 flex -translate-x-1/2"
        style={{ left: x(props.value), padding: `${FRAME + FRAME_GAP}px` }}
      >
        <span
          class="relative flex min-w-[40px] items-center justify-center px-[4px]"
          style={look().style}
        >
          <span
            class="text-[17px] leading-none font-semibold tabular-nums"
            style={{ color: look().ink }}
          >
            {fmt(props.setting.steps[props.value])}
          </span>
          <Marks look={look()} />
        </span>
      </span>
    </div>
  );
}

/** The value printed at the row's end, for forms whose scale carries no number. */
function ValueLabel(props: SliderProps & { class: string }) {
  const look = () =>
    props.design.sliderColors(props.focused).value({ focused: props.focused, changed: props.changed });
  return (
    <span
      class={`flex w-[46px] shrink-0 items-center justify-end pr-[8px] tabular-nums ${props.class}`}
      style={{ color: look().ink, ...look().style }}
    >
      {fmt(props.setting.steps[props.value])}
    </span>
  );
}

/** An exposure scale: ticks, a longer tick at zero, and a full-height needle
 * on the value; the number stands at the row's end. */
function RulerSlider(props: SliderProps) {
  const colors = () => props.design.sliderColors(props.focused);
  const count = () => props.setting.steps.length;
  const zero = () => props.setting.steps.indexOf(0);
  const inset = 10;
  const x = (i: number) => `calc(${inset}px + (100% - ${2 * inset}px) * ${i / (count() - 1)})`;
  return (
    <div class="flex flex-1 items-stretch">
      <div class="relative flex-1" onClick={(e) => pickStep(e, count(), inset, props.pick)}>
        <div
          class="absolute bottom-[12px] h-[1px]"
          style={{ left: `${inset}px`, right: `${inset}px`, background: colors().track }}
        />
        <For each={props.setting.steps.map((step, i) => i)}>
          {(i) => (
            <span
              class="absolute bottom-[12px] -translate-x-1/2"
              style={{
                left: x(i),
                width: i === zero() ? '2px' : '1px',
                height: i === zero() ? '16px' : '8px',
                background: i === zero() ? colors().zero : colors().tick,
              }}
            />
          )}
        </For>
        <span
          class="absolute bottom-[3px] -translate-x-1/2 border-x-[6px] border-b-[8px] border-x-transparent"
          style={{ left: x(props.value), 'border-bottom-color': colors().fill }}
        />
        <span
          class="absolute top-[5px] bottom-[12px] w-[3px] -translate-x-1/2"
          style={{ left: x(props.value), background: colors().fill }}
        />
      </div>
      <ValueLabel {...props} class="text-[19px] font-normal" />
    </div>
  );
}

/** Every step printed, as on a dial; the value large and bright. */
function DigitSlider(props: SliderProps) {
  const colors = () => props.design.sliderColors(props.focused);
  const look = () => colors().value({ focused: props.focused, changed: props.changed });
  const short = (step: Step) => (typeof step === 'string' ? step : fmt(step).replace('+', ''));
  /** Five steps around the value, as much of the scale as a dial window shows. */
  const shown = () => {
    const steps = props.setting.steps.map((step, i) => ({ step, i }));
    const first = Math.max(0, Math.min(steps.length - 5, props.value - 2));
    return steps.slice(first, first + 5);
  };
  return (
    <div class="flex flex-1 items-center justify-between px-[8px]">
      <For each={shown()}>
        {(item) => (
          <span
            class="relative flex h-full items-center justify-center tabular-nums"
            style={{
              color: item.i === props.value ? look().ink : colors().other,
              'font-size': item.i === props.value ? '21px' : '13px',
              'font-weight': item.i === props.value ? 700 : 500,
              'min-width': item.i === props.value ? '26px' : undefined,
            }}
            onClick={(e) => {
              e.stopPropagation();
              props.pick(item.i);
            }}
          >
            {item.i === props.value ? fmt(item.step) : short(item.step)}
          </span>
        )}
      </For>
    </div>
  );
}

/** A segment meter: one block per step, lit from zero to the value. */
function BlockSlider(props: SliderProps) {
  const colors = () => props.design.sliderColors(props.focused);
  const zero = () => props.setting.steps.indexOf(0);
  const lit = (i: number) =>
    (i >= Math.min(zero(), props.value) && i <= Math.max(zero(), props.value) && i !== zero()) ||
    i === props.value;
  return (
    <div class="flex flex-1 items-stretch">
      <div class="flex flex-1 items-center gap-[3px] pl-[8px]">
        <For each={props.setting.steps.map((step, i) => i)}>
          {(i) => (
            <span
              class="h-[18px] flex-1"
              style={{
                background: lit(i) ? colors().fill : colors().track,
                'box-shadow': i === zero() ? ring(1, colors().zero) : undefined,
                height: i === props.value ? '24px' : '18px',
              }}
              onClick={(e) => {
                e.stopPropagation();
                props.pick(i);
              }}
            />
          )}
        </For>
      </div>
      <ValueLabel {...props} class="text-[18px] font-medium" />
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
  design: Design;
  side?: Side;
  send: Send;
}) {
  const d = () => props.design;
  const controlWidth = () => props.width - ICON_COLUMN - 1;
  const pick = (value: number) =>
    props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  return (
    <div
      class="relative flex items-stretch"
      style={{ height: `${ROW}px`, ...d().row(props.focused) }}
      onClick={() => props.send({ focus: props.index, side: props.side })}
    >
      <div
        class="flex shrink-0 items-center justify-center"
        style={{
          width: `${ICON_COLUMN}px`,
          'border-right': `1px solid ${d().divider?.(props.focused) ?? 'transparent'}`,
          'padding-bottom': `${d().lines.bottom - d().lines.top}px`,
        }}
      >
        <RowIconView icon={props.setting.icon} color={d().iconInk(props.focused)} />
      </div>
      <div
        class="flex min-w-0 flex-1 items-stretch"
        style={{
          background: d().drawer?.(props.focused),
          'padding-top': `${d().lines.top}px`,
          'padding-bottom': `${d().lines.bottom}px`,
        }}
      >
        <Show
          when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
          fallback={
            <ChoiceStrip
              setting={props.setting as ChoiceSetting}
              value={props.recipe[props.setting.id]}
              width={controlWidth()}
              design={d()}
              focused={props.focused}
              changed={props.changed}
              pick={pick}
            />
          }
        >
          {(slider) => (
            <SliderView
              setting={slider()}
              value={props.recipe[slider().id]}
              design={d()}
              focused={props.focused}
              changed={props.changed}
              pick={pick}
            />
          )}
        </Show>
      </div>
      <Show when={props.focused}>
        <Show when={d().focus === 'stripe'}>
          <span
            class="pointer-events-none absolute top-[1px] bottom-[2px] left-0"
            style={{ width: `${STRIPE}px`, background: d().focusColor }}
          />
        </Show>
        <Show when={d().focus === 'frame'}>
          <span
            class="pointer-events-none absolute inset-0"
            style={{ 'box-shadow': ring(2, d().focusColor) }}
          />
        </Show>
        <Show when={d().focus === 'corners'}>
          <Corners arm={12} weight={3} color={d().focusColor} />
        </Show>
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
  design: Design;
  side?: Side;
  active?: boolean;
  send: Send;
}) {
  const height = () => 480;
  const scroll = () => scrollFor(props.focus, height());
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
        class="relative flex-1 overflow-hidden"
        style={{
          filter: props.design.filter,
          'mask-image': `linear-gradient(to bottom, ${scroll() > 0 ? 'transparent' : '#000'}, #000 ${FADE}px, #000 calc(100% - ${FADE}px), ${scroll() < SETTINGS.length * ROW - height() ? 'transparent' : '#000'})`,
        }}
      >
        <div style={{ transform: `translateY(${-scroll()}px)` }}>
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
                design={props.design}
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
  design: DesignId;
}

function valueName(setting: Setting, recipe: Recipe): string {
  return setting.kind === 'choice'
    ? setting.choices[recipe[setting.id]].name
    : (STEP_NAMES[String(setting.steps[recipe[setting.id]])] ??
        fmt(setting.steps[recipe[setting.id]]));
}

function Editor(props: { state: EditorState; send?: Send }) {
  const s = () => props.state;
  const send: Send = (action) => props.send?.(action);
  const design = () => DESIGNS[s().design];
  const focused = () => SETTINGS[s().focus];
  const rowTop = () => s().focus * ROW - scrollFor(s().focus, 480);
  const colors = palette(null);
  const compare = () => (s().comparing ? s().snapshot : undefined);

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
        '--accent': s().mode === 'raw' ? 'var(--color-play)' : 'var(--color-rec)',
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
            <span
              class={`px-[12px] py-[6px] text-[22px] ${design().card.class}`}
              style={design().card.style}
            >
              {focused().name}
            </span>
            <span
              class={`px-[12px] py-[4px] text-[22px] ${design().card.class}`}
              style={design().card.style}
            >
              {valueName(focused(), s().recipe)}
            </span>
          </div>
        </Show>
        <Show when={s().mode === 'raw'}>
          <span class="absolute top-[10px] right-[12px] bg-(--accent) px-[8px] py-[2px] text-[17px] font-bold">
            RAW
          </span>
        </Show>
        <Show when={s().fly === 'film'}>
          <RowExtension
            top={rowTop()}
            left={WIDTH}
            width={300}
            design={design()}
            title={FILMS[s().recipe.film].name}
            bodyHeight={104}
            onClick={() => send({ key: 'i' })}
          >
            <p class="text-[19px] leading-[27px] font-light tracking-[0.04em]">
              {FILMS[s().recipe.film].note}
            </p>
          </RowExtension>
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
  focus: 1,
  fly: 'none',
  comparing: false,
  side: 'b',
  design: 'embossed',
};

const PADS = [
  { label: '◀', key: 'ArrowLeft' },
  { label: '▲', key: 'ArrowUp' },
  { label: '▼', key: 'ArrowDown' },
  { label: '▶', key: 'ArrowRight' },
  { label: 'OK', key: 'Enter' },
  { label: 'BACK', key: 'Escape' },
];

/** One of several page options, as a single segmented control. */
function Segmented<T extends string>(props: {
  choices: { value: T; label: string }[];
  value: T;
  pick: (value: T) => void;
}) {
  return (
    <div class="flex rounded-[12px] bg-[#1e1e20] p-[3px]">
      <For each={props.choices}>
        {(choice) => (
          <button
            type="button"
            class="min-h-[38px] rounded-[9px] px-[14px] text-[15px] font-semibold"
            style={{
              background: props.value === choice.value ? '#0068d8' : undefined,
              color: props.value === choice.value ? '#fff' : '#aaa',
            }}
            onClick={() => props.pick(choice.value)}
          >
            {choice.label}
          </button>
        )}
      </For>
    </div>
  );
}

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
      <div class="mb-[10px] flex flex-wrap gap-[10px]">
        <Segmented
          choices={(Object.keys(DESIGNS) as DesignId[]).map((id) => ({
            value: id,
            label: DESIGNS[id].label,
          }))}
          value={state().design}
          pick={(design) => setState({ ...state(), design })}
        />
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
  shadow: at(LEVELS, 2),
  highlight: at(LEVELS, 1),
  color: at(LEVELS, -1),
  nr: at(NR_LEVELS, 'SNR'),
  grain: 2,
  shiftR: 4,
  shiftB: -5,
};

/** Each design with the WB row focused and changed from A, beside other
 * changed and unchanged rows. */
const DESIGN_STATES = (Object.keys(DESIGNS) as DesignId[]).flatMap((design) => [
  {
    id: design,
    title: DESIGNS[design].label,
    state: {
      ...INITIAL,
      design,
      snapshot: BASE,
      recipe: { ...EDITED, wb: 1 },
      focus: 1,
    } satisfies EditorState,
  },
  {
    id: `${design}-compare`,
    title: `${DESIGNS[design].label}, compare`,
    state: {
      ...INITIAL,
      design,
      snapshot: BASE,
      recipe: EDITED,
      focus: 4,
      comparing: true,
    } satisfies EditorState,
  },
]);

const STATES: { id: string; title: string; state: EditorState }[] = [
  ...DESIGN_STATES,
  { id: 'slider', title: 'Slider row', state: { ...INITIAL, focus: 4 } },
  {
    id: 'nr',
    title: 'NOISE REDUCTION with the levels below -2',
    state: { ...INITIAL, focus: 7, recipe: EDITED },
  },
  { id: 'film-info', title: 'Film simulation notes', state: { ...INITIAL, focus: 0, fly: 'film' } },
  { id: 'wb-shift', title: 'WB SHIFT', state: { ...INITIAL, focus: 1, fly: 'shift' } },
  {
    id: 'compare',
    title: 'Compare, editing B',
    state: { ...INITIAL, snapshot: BASE, recipe: EDITED, focus: 4, comparing: true },
  },
  {
    id: 'raw',
    title: 'RAW CONVERSION in playback',
    state: { ...INITIAL, mode: 'raw', recipe: EDITED, focus: 5 },
  },
];

function Review() {
  return (
    <div class="min-h-dvh touch-manipulation bg-[#111] p-[16px] font-sans text-[15px] text-[#ddd]">
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
