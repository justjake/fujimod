import { render } from '@solidjs/web';
import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

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

/* Five whole designs of the same layout, beside the embossed one built first.
   Each gives a row's choices a physical object that plainly scrolls sideways
   (a recessed channel under a loupe, a drum, a film strip, a row of keys, a
   small LCD), sets sliders in the same channel, and sets every other surface: the column, row focus, icons, the
   title box, the legend and the fly-outs. */

type DesignId =
  | 'embossed'
  | 'loupe'
  | 'drum'
  | 'film'
  | 'keys'
  | 'lcd'
  | 'mono'
  | 'glass'
  | 'wheel'
  | 'hasselblad';

/** How a drawer shows that its choices continue past its ends. */
type Depth = 'shadow' | 'fade' | 'clip';

/** Mockup options shared by every design. */
const [center, setCenter] = createSignal(true);
const [depth, setDepth] = createSignal<Depth>('shadow');
/** The accent for focus and changes outside RAW CONVERSION, which is playback's. */
type Accent = 'custom' | 'rec' | 'setup';
const ACCENTS: Record<Accent, { label: string; color: string }> = {
  custom: { label: 'Custom purple', color: '#7d3fc2' },
  rec: { label: 'Shooting red', color: 'var(--color-rec)' },
  setup: { label: 'Setup blue', color: 'var(--color-setup)' },
};
const [accent, setAccent] = createSignal<Accent>('custom');

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

/** A cell or a thumb's box: its own surface and edges, its ink, and marks. */
interface Look {
  style: JSX.CSSProperties;
  ink: string;
  corners?: CornerMarks;
  /** A small square in the top right corner. */
  pip?: string;
  /** A small dot under the middle. */
  dot?: string;
}

/** One cell of a strip: whether it is the chosen one, how many cells away
 * from the chosen one it sits, and its row's state. */
interface CellState extends Mark {
  chosen: boolean;
  distance: number;
}

/** The sideways-scrolling object a row's choices (and, in most designs, its
 * slider steps) sit in. */
interface Strip {
  /** The fixed channel the strip runs in. */
  track: (focused: boolean) => JSX.CSSProperties;
  /** The strip's own surface, which slides with its cells. */
  film?: JSX.CSSProperties;
  /** Space between the channel's edges and the cells. */
  inset: number;
  gap: number;
  /** A word cell's side padding and least width. */
  pad: number;
  min: number;
  font: string;
  cell: (state: CellState) => Look;
  /** The chevron at an end with more cells beyond it, its halo, and the
   * shadow cast over cells at that end. */
  ends: { ink: string; halo?: string; shadow: string };
}

/** The FUJIMOD SET-UP slider: a track, zero to the value filled, a thumb. */
interface ThumbColors {
  track: string;
  tick: string;
  zero: string;
  fill: string;
  value: (mark: Mark) => Look;
}

/** A ruler slider: a tick scale with each step labeled. The scale slides
 * under a fixed needle in the middle, or stays put while the needle moves. */
interface Ruler {
  kind: 'ruler';
  fixed: boolean;
  /** Between steps, when the scale slides. */
  spacing: number;
  tick: string;
  major: string;
  /** The ticks from zero to the value. */
  fill: string;
  /** Unchosen step labels; none hides them. */
  label?: string;
  /** The needle and the chosen step's label. */
  chosen: (mark: Mark) => string;
  font: string;
}

/** Title case, for designs that do not write in capitals. */
const titleCase = (name: string) =>
  name.toLowerCase().replace(/(^|[\s+-])(\S)/g, (_, gap: string, c: string) => gap + c.toUpperCase());

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
  strip: Strip;
  /** The slider, set in the strip's channel. */
  slider: ThumbColors | Ruler;
  card: Card;
  legend: { box: JSX.CSSProperties; key: JSX.CSSProperties };
  /** The fly-out's body below its band; the band is the focused row. */
  body: JSX.CSSProperties;
}

const ACCENT = 'var(--accent)';
const ring = (width: number, color: string) => `inset 0 0 0 ${width}px ${color}`;
const top = (width: number, color: string) => `inset 0 ${width}px 0 ${color}`;
const bottom = (width: number, color: string) => `inset 0 -${width}px 0 ${color}`;
/** Box shadows listed first paint on top; false entries drop out. */
const edges = (...list: (string | false | undefined)[]) =>
  list.filter((edge): edge is string => typeof edge === 'string').join(', ') || undefined;
/** Cells fade with distance from the chosen one. */
const fade = (distance: number, steps: number[]) => steps[Math.min(distance, steps.length - 1)];

/** A raised part: lit top edge, dark bottom edge and a hard shadow below. */
const RAISED = 'inset 0 1px 0 rgb(255 255 255 / 0.4), inset 0 -2px 0 rgb(0 0 0 / 0.55), 0 2px 0 #000';
/** A recessed channel: shadowed top edge, lit bottom edge. */
const RECESSED = 'inset 0 3px 0 #000, inset 0 -1px 0 rgb(255 255 255 / 0.22)';

const FILM_BASE = '#2a1d10';
const FILM_EDGE = '#120b04';
const FILM_LIT = '#f4ead4';
const LCD = '#a6b19a';
const LCD_LIT = '#c9d6b8';
const LCD_INK = '#141a10';
const MONO = '#b4b4b0';
const MONO_INK = '#101010';

const BASE_DESIGNS: Record<Exclude<DesignId, 'mono' | 'glass' | 'wheel' | 'hasselblad'>, Design> = {
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
    strip: {
      track: () => ({ background: 'rgb(255 255 255 / 0.11)' }),
      inset: 0,
      gap: 0,
      pad: 10,
      min: 0,
      font: 'text-[16px] font-light tracking-[0.08em] uppercase',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: 'rgb(255 255 255 / 0.2)',
                'box-shadow': edges(c.changed && bottom(3, ACCENT), c.focused && ring(3, '#fff')),
              },
              ink: '#fff',
            }
          : { style: {}, ink: '#fff' },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.55)', shadow: 'rgb(0 0 0 / 0.85)' },
    },
    slider: {
      track: 'rgb(255 255 255 / 0.35)',
      tick: 'rgb(255 255 255 / 0.55)',
      zero: '#fff',
      fill: ACCENT,
      value: (m) => ({
        style: { background: '#fff', 'box-shadow': m.focused ? ring(3, '#fff') : undefined },
        ink: m.changed ? ACCENT : '#000',
      }),
    },
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

  /* A loupe over a recessed channel: the choices slide along a dark groove,
     and the chosen one sits under a raised, beveled window in the middle.
     Sliders run the same way, their steps passing under the window. */
  loupe: {
    label: 'Loupe',
    panel: { background: 'rgb(40 40 43 / 0.92)' },
    edge: (side) =>
      side === 'right'
        ? 'inset -1px 0 0 rgb(255 255 255 / 0.25), inset -3px 0 0 #000'
        : 'inset 1px 0 0 rgb(255 255 255 / 0.25), inset 3px 0 0 #000',
    row: (focused) => ({
      background: focused ? 'rgb(84 84 90 / 0.97)' : undefined,
      'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.16), inset 0 -2px 0 #000',
    }),
    lines: { top: 1, bottom: 2 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    strip: {
      track: (focused) => ({
        background: focused ? '#141416' : '#0c0c0d',
        'box-shadow': RECESSED,
      }),
      inset: 4,
      gap: 2,
      pad: 9,
      min: 34,
      font: 'text-[16px] font-normal tracking-[0.08em] uppercase',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: c.focused ? '#5e5e64' : '#3e3e42',
                'box-shadow': edges(
                  c.focused && ring(2, '#fff'),
                  RAISED,
                  c.changed && bottom(3, ACCENT),
                ),
              },
              ink: '#fff',
            }
          : { style: { opacity: fade(c.distance, [1, 0.62, 0.4, 0.26]) }, ink: '#fff' },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.55)', shadow: '#000' },
    },
    slider: {
      track: 'rgb(255 255 255 / 0.22)',
      tick: 'rgb(255 255 255 / 0.4)',
      zero: '#fff',
      fill: ACCENT,
      value: (m) => ({
        style: {
          background: m.focused ? '#5e5e64' : '#3e3e42',
          'box-shadow': edges(m.focused && ring(2, '#fff'), RAISED, m.changed && bottom(3, ACCENT)),
        },
        ink: '#fff',
      }),
    },
    card: {
      style: { background: 'rgb(40 40 43 / 0.94)', color: '#fff', 'box-shadow': RAISED },
      class: 'font-light tracking-[0.14em] uppercase',
    },
    legend: {
      box: { background: 'rgb(40 40 43 / 0.94)', color: '#fff', 'box-shadow': RAISED },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(40 40 43 / 0.97)', color: '#fff', 'box-shadow': RAISED },
  },

  /* A drum: choices printed round a barrel that turns in a slot, shrinking and
     darkening toward its ends. Rails above and below the middle mark the
     chosen one. */
  drum: {
    label: 'Drum',
    panel: { background: 'rgb(18 18 20 / 0.9)' },
    edge: (side) => (side === 'right' ? 'inset -2px 0 0 #000' : 'inset 2px 0 0 #000'),
    row: (focused) => ({
      background: focused ? 'rgb(52 52 56 / 0.96)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.1)',
    }),
    lines: { top: 0, bottom: 1 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    divider: () => 'rgb(255 255 255 / 0.14)',
    strip: {
      track: (focused) => ({
        background: focused ? '#2c2c30' : '#1d1d20',
        'box-shadow': edges(
          'inset 0 2px 0 #000',
          'inset 0 -2px 0 #000',
          'inset 14px 0 0 rgb(0 0 0 / 0.45)',
          'inset -14px 0 0 rgb(0 0 0 / 0.45)',
          'inset 30px 0 0 rgb(0 0 0 / 0.25)',
          'inset -30px 0 0 rgb(0 0 0 / 0.25)',
        ),
      }),
      inset: 3,
      gap: 0,
      pad: 8,
      min: 32,
      font: 'text-[17px] font-medium tracking-[0.05em] uppercase',
      cell: (c) => {
        const scale = fade(c.distance, [1, 0.84, 0.72, 0.64]);
        const rail = c.changed ? ACCENT : c.focused ? '#fff' : 'rgb(255 255 255 / 0.7)';
        return c.chosen
          ? {
              style: {
                'box-shadow': edges(top(2, rail), bottom(2, rail)),
                background: c.focused ? 'rgb(255 255 255 / 0.1)' : undefined,
              },
              ink: '#fff',
            }
          : {
              style: {
                transform: `scale(${scale})`,
                opacity: fade(c.distance, [1, 0.6, 0.38, 0.24]),
              },
              ink: '#fff',
            };
      },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.55)', shadow: 'rgb(0 0 0 / 0.85)' },
    },
    slider: {
      track: 'rgb(255 255 255 / 0.3)',
      tick: 'rgb(255 255 255 / 0.45)',
      zero: '#fff',
      fill: ACCENT,
      value: (m) => {
        const rail = m.changed ? ACCENT : m.focused ? '#fff' : 'rgb(255 255 255 / 0.7)';
        return {
          style: {
            background: m.focused ? '#3a3a3f' : '#1d1d20',
            'box-shadow': edges(top(2, rail), bottom(2, rail)),
          },
          ink: '#fff',
        };
      },
    },
    card: {
      style: { background: 'rgb(18 18 20 / 0.9)', color: '#fff', 'box-shadow': edges(top(2, '#fff'), bottom(2, '#fff')) },
      class: 'font-medium tracking-[0.12em] uppercase',
    },
    legend: {
      box: { background: 'rgb(18 18 20 / 0.9)', color: '#fff' },
      key: { background: '#fff', color: '#000' },
    },
    body: { background: 'rgb(18 18 20 / 0.96)', color: '#fff', 'box-shadow': ring(2, '#000') },
  },

  /* Film: each row's choices are frames on a strip of film, sprocket holes
     running past above and below. The chosen frame is lit as on a light
     table. */
  film: {
    label: 'Film strip',
    panel: { background: 'rgb(16 12 8 / 0.9)' },
    edge: (side) => (side === 'right' ? 'inset -2px 0 0 #000' : 'inset 2px 0 0 #000'),
    row: (focused) => ({
      background: focused ? 'rgb(64 50 34 / 0.95)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 230 190 / 0.1)',
    }),
    lines: { top: 0, bottom: 1 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => FILM_LIT,
    strip: {
      track: () => ({ background: FILM_EDGE }),
      film: {
        background: FILM_BASE,
        'background-image': `repeating-linear-gradient(to right, transparent 0 3px, ${FILM_EDGE} 3px 7px, transparent 7px 10px), repeating-linear-gradient(to right, transparent 0 3px, ${FILM_EDGE} 3px 7px, transparent 7px 10px)`,
        'background-size': '10px 4px, 10px 4px',
        'background-position': '0 3px, 0 calc(100% - 3px)',
        'background-repeat': 'repeat-x',
      },
      inset: 9,
      gap: 3,
      pad: 8,
      min: 34,
      font: 'text-[15px] font-medium tracking-[0.06em] uppercase',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: FILM_LIT,
                'box-shadow': edges(c.focused && ring(2, ACCENT), c.changed && bottom(3, ACCENT)),
              },
              ink: FILM_EDGE,
            }
          : {
              style: { background: 'rgb(255 230 190 / 0.08)' },
              ink: `rgb(244 234 212 / ${fade(c.distance, [1, 0.75, 0.55, 0.4])})`,
            },
      ends: { ink: FILM_LIT, halo: 'rgb(0 0 0 / 0.55)', shadow: FILM_EDGE },
    },
    slider: {
      track: 'rgb(244 234 212 / 0.3)',
      tick: 'rgb(244 234 212 / 0.5)',
      zero: FILM_LIT,
      fill: ACCENT,
      value: (m) => ({
        style: {
          background: FILM_LIT,
          'box-shadow': edges(m.focused && ring(2, ACCENT), m.changed && bottom(3, ACCENT)),
        },
        ink: FILM_EDGE,
      }),
    },
    card: {
      style: { background: FILM_LIT, color: FILM_EDGE },
      class: 'font-medium tracking-[0.12em] uppercase',
    },
    legend: {
      box: { background: 'rgb(16 12 8 / 0.9)', color: FILM_LIT },
      key: { background: FILM_LIT, color: FILM_EDGE },
    },
    body: { background: 'rgb(30 22 14 / 0.97)', color: FILM_LIT, 'box-shadow': ring(2, FILM_EDGE) },
  },

  /* Keys: each choice a raised key in a tray; the chosen key is pressed in and
     its lamp is lit. Sliders are a row of numbered keys. */
  keys: {
    label: 'Keys',
    panel: { background: 'rgb(26 26 28 / 0.92)' },
    edge: (side) => (side === 'right' ? 'inset -2px 0 0 #000' : 'inset 2px 0 0 #000'),
    row: (focused) => ({
      background: focused ? 'rgb(58 58 62 / 0.96)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(0 0 0 / 0.9), inset 0 1px 0 rgb(255 255 255 / 0.07)',
    }),
    lines: { top: 1, bottom: 1 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    divider: () => 'rgb(0 0 0 / 0.8)',
    strip: {
      track: () => ({ background: '#0a0a0b', 'box-shadow': 'inset 0 2px 0 #000' }),
      inset: 4,
      gap: 4,
      pad: 8,
      min: 30,
      font: 'text-[15px] font-semibold tracking-[0.05em] uppercase',
      cell: (c) => {
        const lamp = c.changed ? ACCENT : c.focused ? '#fff' : 'rgb(255 255 255 / 0.75)';
        return c.chosen
          ? {
              style: {
                background: '#202023',
                transform: 'translateY(2px)',
                'box-shadow': edges(
                  top(3, lamp),
                  c.focused && ring(1, '#fff'),
                  'inset 0 4px 0 #000',
                ),
              },
              ink: '#fff',
            }
          : {
              style: {
                background: '#3b3b3f',
                'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.3), inset 0 -3px 0 #232326, 0 2px 0 #000',
              },
              ink: 'rgb(255 255 255 / 0.85)',
            };
      },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.55)', shadow: '#000' },
    },
    slider: {
      track: 'rgb(255 255 255 / 0.2)',
      tick: 'rgb(255 255 255 / 0.4)',
      zero: '#fff',
      fill: ACCENT,
      value: (m) => ({
        style: {
          background: '#3b3b3f',
          'box-shadow': edges(
            top(3, m.changed ? ACCENT : m.focused ? '#fff' : 'rgb(255 255 255 / 0.75)'),
            m.focused && ring(1, '#fff'),
            'inset 0 -3px 0 #232326, 0 2px 0 #000',
          ),
        },
        ink: '#fff',
      }),
    },
    card: {
      style: {
        background: '#3b3b3f',
        color: '#fff',
        'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.3), inset 0 -3px 0 #232326',
      },
      class: 'font-semibold tracking-[0.1em] uppercase',
    },
    legend: {
      box: {
        background: '#3b3b3f',
        color: '#fff',
        'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.3), inset 0 -3px 0 #232326',
      },
      key: { background: '#0a0a0b', color: '#fff' },
    },
    body: { background: 'rgb(26 26 28 / 0.97)', color: '#fff', 'box-shadow': ring(2, '#000') },
  },

  /* The top LCD: each row's choices on a small reflective LCD set into the
     body, unchosen choices as faint ghost segments, the chosen one solid and
     inverted. The focused row's LCD lights up. */
  lcd: {
    label: 'Top LCD',
    panel: { background: 'rgb(22 22 22 / 0.92)' },
    edge: (side) => (side === 'right' ? 'inset -2px 0 0 #000' : 'inset 2px 0 0 #000'),
    row: (focused) => ({
      background: focused ? 'rgb(54 54 54 / 0.96)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.08)',
    }),
    lines: { top: 0, bottom: 1 },
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    strip: {
      track: (focused) => ({
        background: focused ? LCD_LIT : LCD,
        'box-shadow': edges(ring(2, '#050505'), 'inset 0 4px 0 rgb(0 0 0 / 0.28)'),
      }),
      inset: 6,
      gap: 2,
      pad: 7,
      min: 28,
      font: 'text-[16px] font-bold tracking-[0.04em] uppercase tabular-nums',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: LCD_INK,
                'box-shadow': c.changed ? bottom(3, ACCENT) : undefined,
              },
              ink: c.focused ? LCD_LIT : LCD,
            }
          : { style: {}, ink: 'rgb(20 26 16 / 0.32)' },
      ends: { ink: LCD_INK, shadow: 'rgb(20 26 16 / 0.45)' },
    },
    slider: {
      track: 'rgb(20 26 16 / 0.35)',
      tick: 'rgb(20 26 16 / 0.5)',
      zero: LCD_INK,
      fill: LCD_INK,
      value: (m) => ({
        style: { background: LCD_INK, 'box-shadow': m.changed ? bottom(3, ACCENT) : undefined },
        ink: m.focused ? LCD_LIT : LCD,
      }),
    },
    card: {
      style: { background: LCD_LIT, color: LCD_INK, 'box-shadow': ring(2, '#050505') },
      class: 'font-bold tracking-[0.08em] uppercase',
    },
    legend: {
      box: { background: 'rgb(22 22 22 / 0.92)', color: '#fff' },
      key: { background: LCD_LIT, color: LCD_INK },
    },
    body: { background: LCD_LIT, color: LCD_INK, 'box-shadow': ring(2, '#050505') },
  },
};

/* The top LCD in monochrome: gray segments, and the focused row's LCD
   outlined in the shooting accent rather than lit. */
/** A pane of dark glass, lit along its top edge. */
const GLASS: JSX.CSSProperties = {
  background: 'rgb(30 30 32 / 0.52)',
  'backdrop-filter': 'blur(24px) saturate(1.15)',
  'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.28), inset 0 0 0 1px rgb(255 255 255 / 0.1)',
};

const DESIGNS: Record<DesignId, Design> = {
  ...BASE_DESIGNS,
  mono: {
  ...BASE_DESIGNS.lcd,
  label: 'Mono LCD',
  focus: 'none',
  strip: {
    ...BASE_DESIGNS.lcd.strip,
    track: (focused) => ({
      background: MONO,
      'box-shadow': edges(
        focused ? ring(3, ACCENT) : ring(2, '#050505'),
        'inset 0 4px 0 rgb(0 0 0 / 0.28)',
      ),
    }),
    cell: (c) =>
      c.chosen
        ? {
            style: { background: MONO_INK, 'box-shadow': c.changed ? bottom(3, ACCENT) : undefined },
            ink: MONO,
          }
        : { style: {}, ink: 'rgb(16 16 16 / 0.32)' },
    ends: { ink: MONO_INK, shadow: 'rgb(16 16 16 / 0.45)' },
  },
  slider: {
    track: 'rgb(16 16 16 / 0.35)',
    tick: 'rgb(16 16 16 / 0.5)',
    zero: MONO_INK,
    fill: MONO_INK,
    value: (m) => ({
      style: { background: MONO_INK, 'box-shadow': m.changed ? bottom(3, ACCENT) : undefined },
      ink: MONO,
    }),
  },
  card: { ...BASE_DESIGNS.lcd.card, style: { background: MONO, color: MONO_INK, 'box-shadow': ring(2, '#050505') } },
  legend: { ...BASE_DESIGNS.lcd.legend, key: { background: MONO, color: MONO_INK } },
  body: { background: MONO, color: MONO_INK, 'box-shadow': ring(2, '#050505') },

  },

  /* Apple, Liquid Glass: the column a pane of dark glass, each row's choices
     on a rounded track that brightens with focus, the chosen one a white
     capsule (the accent when changed). Sliders are rulers sliding under a
     fixed needle. Names in title case. */
  glass: {
    label: 'Glass',
    panel: { background: 'rgb(22 22 24 / 0.5)', 'backdrop-filter': 'blur(24px) saturate(1.15)' },
    edge: (side) =>
      side === 'right' ? 'inset -1px 0 0 rgb(255 255 255 / 0.22)' : 'inset 1px 0 0 rgb(255 255 255 / 0.22)',
    row: () => ({}),
    lines: { top: 3, bottom: 3, right: 6 },
    text: titleCase,
    focus: 'none',
    focusColor: ACCENT,
    iconInk: (focused) => (focused ? '#fff' : 'rgb(255 255 255 / 0.78)'),
    strip: {
      track: (focused) => ({
        background: focused ? 'rgb(255 255 255 / 0.17)' : 'rgb(255 255 255 / 0.06)',
        'border-radius': '13px',
        'box-shadow': focused
          ? 'inset 0 1px 0 rgb(255 255 255 / 0.35), inset 0 0 0 1px rgb(255 255 255 / 0.14)'
          : undefined,
      }),
      inset: 4,
      gap: 2,
      pad: 11,
      min: 30,
      font: 'text-[16px] font-medium',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: c.changed ? ACCENT : '#fff',
                'border-radius': '10px',
                'box-shadow': '0 1px 5px rgb(0 0 0 / 0.35)',
              },
              ink: c.changed ? '#fff' : '#000',
            }
          : { style: {}, ink: `rgb(255 255 255 / ${fade(c.distance, [0.92, 0.66, 0.45, 0.3])})` },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.5)', shadow: 'rgb(0 0 0 / 0.3)' },
    },
    slider: {
      kind: 'ruler',
      fixed: false,
      spacing: 38,
      tick: 'rgb(255 255 255 / 0.3)',
      major: 'rgb(255 255 255 / 0.65)',
      fill: ACCENT,
      label: 'rgb(255 255 255 / 0.5)',
      chosen: (m) => (m.changed ? ACCENT : '#fff'),
      font: 'text-[12px] font-medium',
    },
    card: {
      style: { ...GLASS, 'border-radius': '14px', color: '#fff' },
      class: 'font-semibold',
    },
    legend: {
      box: { ...GLASS, 'border-radius': '12px', color: '#fff' },
      key: { background: '#fff', color: '#000', 'border-radius': '6px' },
    },
    body: { ...GLASS, 'border-radius': '16px', color: '#fff' },
  },

  /* The iPhone Camera's mode dial: each row's choices a line of words, the
     chosen one held in the middle in the accent while the row has focus, a
     dot under it when changed. The focused row's icon turns the accent too.
     Sliders are rulers under a fixed needle. */
  wheel: {
    label: 'Camera wheel',
    panel: { background: 'rgb(0 0 0 / 0.55)' },
    edge: () => undefined,
    row: (focused) => ({ background: focused ? 'rgb(255 255 255 / 0.09)' : undefined }),
    lines: { top: 0, bottom: 0 },
    focus: 'none',
    focusColor: ACCENT,
    iconInk: (focused) => (focused ? ACCENT : '#fff'),
    strip: {
      track: () => ({}),
      inset: 0,
      gap: 0,
      pad: 11,
      min: 30,
      font: 'text-[15px] font-semibold tracking-[0.08em] uppercase',
      cell: (c) =>
        c.chosen
          ? { style: {}, ink: c.focused ? ACCENT : '#fff', dot: c.changed ? ACCENT : undefined }
          : { style: {}, ink: `rgb(255 255 255 / ${fade(c.distance, [0.75, 0.5, 0.32, 0.2])})` },
      ends: { ink: '#fff', halo: 'rgb(0 0 0 / 0.6)', shadow: 'rgb(0 0 0 / 0.5)' },
    },
    slider: {
      kind: 'ruler',
      fixed: false,
      spacing: 34,
      tick: 'rgb(255 255 255 / 0.35)',
      major: 'rgb(255 255 255 / 0.75)',
      fill: '#fff',
      label: 'rgb(255 255 255 / 0.5)',
      chosen: (m) => (m.focused || m.changed ? ACCENT : '#fff'),
      font: 'text-[12px] font-semibold',
    },
    card: {
      style: { background: 'rgb(0 0 0 / 0.5)', 'border-radius': '8px', color: '#fff' },
      class: 'font-semibold tracking-[0.06em] uppercase',
    },
    legend: {
      box: { background: 'rgb(0 0 0 / 0.5)', 'border-radius': '8px', color: '#fff' },
      key: { background: '#fff', color: '#000', 'border-radius': '5px' },
    },
    body: { background: 'rgb(0 0 0 / 0.7)', 'border-radius': '12px', color: '#fff' },
  },

  /* Hasselblad's control screen: black, light type, hairlines; the chosen
     choice outlined as its AF box is, and filled white with focus. Sliders
     are its exposure scale: a fixed scale with the ticks from zero lit and
     the value over the needle. */
  hasselblad: {
    label: 'Hasselblad',
    panel: { background: 'rgb(0 0 0 / 0.8)' },
    edge: (side) => (side === 'right' ? 'inset -1px 0 0 rgb(255 255 255 / 0.18)' : 'inset 1px 0 0 rgb(255 255 255 / 0.18)'),
    row: (focused) => ({
      background: focused ? 'rgb(255 255 255 / 0.1)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.14)',
    }),
    lines: { top: 0, bottom: 1 },
    text: titleCase,
    focus: 'stripe',
    focusColor: ACCENT,
    iconInk: () => '#fff',
    strip: {
      track: () => ({}),
      inset: 6,
      gap: 4,
      pad: 8,
      min: 30,
      font: 'text-[17px] font-light tracking-[0.02em]',
      cell: (c) =>
        c.chosen
          ? {
              style: {
                background: c.focused ? '#fff' : undefined,
                'border-radius': '4px',
                'box-shadow': edges(c.changed && bottom(3, ACCENT), ring(1.5, '#fff')),
              },
              ink: c.focused ? '#000' : '#fff',
            }
          : { style: {}, ink: 'rgb(255 255 255 / 0.5)' },
      ends: { ink: '#fff', shadow: 'rgb(0 0 0 / 0.6)' },
    },
    slider: {
      kind: 'ruler',
      fixed: true,
      spacing: 0,
      tick: 'rgb(255 255 255 / 0.4)',
      major: 'rgb(255 255 255 / 0.7)',
      fill: ACCENT,
      chosen: () => '#fff',
      font: 'text-[12px] font-light',
    },
    card: {
      style: { background: 'rgb(0 0 0 / 0.72)', color: '#fff' },
      class: 'font-light tracking-[0.02em]',
    },
    legend: {
      box: { background: 'rgb(0 0 0 / 0.72)', color: '#fff' },
      key: { background: '#fff', color: '#000', 'border-radius': '4px' },
    },
    body: { background: 'rgb(0 0 0 / 0.85)', color: '#fff' },
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
const FADE_WIDTH = 18;
/** The space at a strip's end that its chevron keeps clear. */
const LANE = 16;
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
      <Show when={props.look.dot}>
        {(color) => (
          <span
            class="pointer-events-none absolute bottom-[3px] left-1/2 h-[5px] w-[5px] -translate-x-1/2 rounded-full"
            style={{ background: color() }}
          />
        )}
      </Show>
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

interface ControlProps {
  design: Design;
  focused: boolean;
  changed: boolean;
  pick: (value: number) => void;
}

interface StripItem {
  name: string;
  icon?: number;
}

/** A row's choices in its design's strip, slid to show the chosen one: held
 * in the middle, or brought just into view from the start. */
function Strip(props: ControlProps & { items: StripItem[]; value: number; width: number }) {
  const strip = () => props.design.strip;
  let track: HTMLDivElement | undefined;
  const [offset, setOffset] = createSignal(0);
  const [before, setBefore] = createSignal(false);
  const [after, setAfter] = createSignal(false);
  /** Space before the first cell and after the last, so either can sit in the middle. */
  const lead = () => (center() ? props.width / 2 : 0);
  const follow = () => {
    const chosen = track?.children[props.value] as HTMLElement | undefined;
    if (!track || !chosen) return;
    const cells = Array.from(track.children) as HTMLElement[];
    let next: number;
    if (center()) {
      next = chosen.offsetLeft + chosen.offsetWidth / 2 - props.width / 2;
    } else {
      // Start on a whole cell, the one that best centers the chosen one.
      const centered = chosen.offsetLeft + chosen.offsetWidth / 2 - props.width / 2;
      const end = chosen.offsetLeft + chosen.offsetWidth;
      next = 0;
      if (track.scrollWidth > props.width)
        for (const cell of cells) {
          const start = cell.offsetLeft - strip().gap;
          if (start > chosen.offsetLeft || end - start > props.width) continue;
          if (Math.abs(start - centered) < Math.abs(next - centered)) next = Math.max(0, start);
        }
    }
    setOffset(next);
    const first = cells[0];
    const last = cells[cells.length - 1];
    setBefore(first.offsetLeft < next);
    setAfter(last.offsetLeft + last.offsetWidth > next + props.width);
  };
  createEffect(
    () => [props.value, props.width, props.design, center()],
    () => {
      requestAnimationFrame(follow);
    },
  );
  /** Cells give way to the chevron's lane at an end with more beyond it:
   * fading out, or cut off at the lane. */
  const fadeMask = () => {
    if (depth() === 'shadow') return undefined;
    const from = LANE;
    const width = depth() === 'fade' ? FADE_WIDTH : 0;
    const end = (more: boolean, side: 'start' | 'end') =>
      side === 'start'
        ? more
          ? `transparent ${from}px, #000 ${from + width}px`
          : '#000 0'
        : more
          ? `#000 calc(100% - ${from + width}px), transparent calc(100% - ${from}px)`
          : '#000 100%';
    return `linear-gradient(to right, ${end(before(), 'start')}, ${end(after(), 'end')})`;
  };
  return (
    <div
      class="relative h-full overflow-hidden"
      style={{ width: `${props.width}px`, ...strip().track(props.focused) }}
    >
      <div class="absolute inset-0 overflow-hidden" style={{ 'mask-image': fadeMask() }}>
        <div
          ref={(element) => (track = element)}
          class="absolute inset-y-0 left-0 flex items-stretch transition-transform duration-150 ease-out"
          style={{
            transform: `translateX(${-offset()}px)`,
            padding: `${strip().inset}px ${lead() || strip().gap}px`,
            gap: `${strip().gap}px`,
            ...strip().film,
          }}
        >
          <For each={props.items.map((item, i) => ({ item, i }))}>
            {(entry) => {
              const look = () =>
                strip().cell({
                  chosen: entry.i === props.value,
                  distance: Math.abs(entry.i - props.value),
                  focused: props.focused,
                  changed: props.changed && entry.i === props.value,
                });
              return (
                <div
                  class="relative flex shrink-0 items-center justify-center transition-transform duration-150"
                  style={{
                    width: entry.item.icon !== undefined ? `${ICON_CELL}px` : undefined,
                    'min-width': `${strip().min}px`,
                    padding: entry.item.icon !== undefined ? undefined : `0 ${strip().pad}px`,
                    ...look().style,
                  }}
                  onClick={(e) => {
                    e.stopPropagation();
                    props.pick(entry.i);
                  }}
                >
                  <Show
                    when={entry.item.icon}
                    fallback={
                      <span class={strip().font} style={{ color: look().ink }}>
                        {props.design.text?.(entry.item.name) ?? entry.item.name}
                      </span>
                    }
                  >
                    {(icon) => (
                      <Ink src={stock(icon())} class="h-[24px] w-[46px]" color={look().ink} />
                    )}
                  </Show>
                  <Marks look={look()} />
                </div>
              );
            }}
          </For>
        </div>
      </div>
      <Show when={depth() === 'shadow'}>
        {/* The drawer is a recess: its walls shade both ends and the top,
            whether or not anything lies past them. */}
        <span
          class="pointer-events-none absolute inset-0"
          style={{
            background: `linear-gradient(to right, ${strip().ends.shadow}, transparent ${LANE + 14}px, transparent calc(100% - ${LANE + 14}px), ${strip().ends.shadow})`,
            'box-shadow': `inset 0 5px 6px -4px ${strip().ends.shadow}`,
          }}
        />
      </Show>
      <For each={['left', 'right'] as const}>
        {(side) => (
          <Show when={side === 'left' ? before() : after()}>
            <span
              class="pointer-events-none absolute inset-y-0 flex items-center"
              style={{ [side]: '3px' }}
            >
              <Chevron side={side} ink={strip().ends.ink} halo={strip().ends.halo} />
            </span>
          </Show>
        )}
      </For>
    </div>
  );
}

/** The toolkit's chevron, small, pointing toward the choices past this end. */
function Chevron(props: { side: 'left' | 'right'; ink: string; halo?: string }) {
  return (
    <svg
      width="9"
      height="16"
      viewBox="0 0 9 16"
      style={{
        color: props.ink,
        filter: props.halo ? `drop-shadow(0 1px 1px ${props.halo})` : undefined,
      }}
    >
      <path
        d={props.side === 'left' ? 'M7 2l-5 6 5 6' : 'M2 2l5 6-5 6'}
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

type SliderProps = ControlProps & { setting: SliderSetting; value: number };

/** FUJIMOD SET-UP's segment bar: zero to the value filled, the value itself a
 * square thumb that carries the number. */
function ThumbSlider(props: SliderProps & { colors: ThumbColors }) {
  const look = () => props.colors.value({ focused: props.focused, changed: props.changed });
  const count = () => props.setting.steps.length;
  const zero = () => props.setting.steps.indexOf(0);
  const x = (i: number) => `calc(${INSET}px + (100% - ${2 * INSET}px) * ${i / (count() - 1)})`;
  return (
    <div
      class="relative mx-[6px] flex-1"
      onClick={(e) => {
        const element = e.currentTarget;
        e.stopPropagation();
        const box = element.getBoundingClientRect();
        const scale = box.width / element.offsetWidth;
        const inner = (e.clientX - box.left) / scale - INSET;
        const i = Math.round((inner / (element.offsetWidth - 2 * INSET)) * (count() - 1));
        props.pick(Math.max(0, Math.min(count() - 1, i)));
      }}
    >
      <div
        class="absolute top-1/2 h-[2px] -translate-y-1/2"
        style={{ left: `${INSET}px`, right: `${INSET}px`, background: props.colors.track }}
      />
      <div
        class="absolute top-1/2 h-[4px] -translate-y-1/2"
        style={{
          left: x(Math.min(zero(), props.value)),
          width: `calc((100% - ${2 * INSET}px) * ${Math.abs(props.value - zero()) / (count() - 1)})`,
          background: props.colors.fill,
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
              background: i === zero() ? props.colors.zero : props.colors.tick,
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

/** One setting's controls, beside its group's icon. */
/** Steps between labeled ticks. */
const RULER_DIVISIONS = 4;

function RulerSlider(props: SliderProps & { ruler: Ruler; width: number }) {
  const ruler = () => props.ruler;
  const mark = () => ({ focused: props.focused, changed: props.changed });
  const count = () => props.setting.steps.length;
  const zero = () => Math.max(0, props.setting.steps.indexOf(0));
  const pad = 26;
  const spacing = () => (ruler().fixed ? (props.width - 2 * pad) / (count() - 1) : ruler().spacing);
  /** Where step 0 sits in the control. */
  const origin = () => (ruler().fixed ? pad : props.width / 2 - props.value * spacing());
  const low = () => Math.min(zero(), props.value);
  const high = () => Math.max(zero(), props.value);
  const ticks = () => Array.from({ length: (count() - 1) * RULER_DIVISIONS + 1 }, (_, t) => t);
  return (
    <div
      class="relative h-full overflow-hidden"
      style={{
        width: `${props.width}px`,
        'mask-image': ruler().fixed
          ? undefined
          : 'linear-gradient(to right, transparent, #000 44px, #000 calc(100% - 44px), transparent)',
      }}
      onClick={(e) => {
        const element = e.currentTarget;
        e.stopPropagation();
        const box = element.getBoundingClientRect();
        const local = ((e.clientX - box.left) / box.width) * element.offsetWidth;
        const i = Math.round((local - origin()) / spacing());
        props.pick(Math.max(0, Math.min(count() - 1, i)));
      }}
    >
      <div
        class="absolute inset-y-0 left-0 transition-transform duration-150 ease-out"
        style={{ transform: `translateX(${origin()}px)` }}
      >
        <For each={ticks()}>
          {(t) => {
            const at = t / RULER_DIVISIONS;
            const major = t % RULER_DIVISIONS === 0;
            const lit = () => at >= low() && at <= high() && high() > low();
            return (
              <span
                class="absolute bottom-[6px] -translate-x-1/2 transition-colors duration-150"
                style={{
                  left: `${at * spacing()}px`,
                  width: major ? '2px' : '1px',
                  height: `${at === zero() ? 15 : major ? 11 : lit() ? 8 : 6}px`,
                  background: lit() ? ruler().fill : major ? ruler().major : ruler().tick,
                }}
              />
            );
          }}
        </For>
        <For each={props.setting.steps}>
          {(step, i) => (
            <Show when={i() !== props.value && ruler().label}>
              {(color) => (
                <span
                  class={`absolute top-[4px] -translate-x-1/2 leading-none tabular-nums ${ruler().font}`}
                  style={{ left: `${i() * spacing()}px`, color: color() }}
                >
                  {fmt(step)}
                </span>
              )}
            </Show>
          )}
        </For>
      </div>
      <span
        class="absolute top-[2px] -translate-x-1/2 text-[16px] leading-none font-semibold tabular-nums transition-[left] duration-150 ease-out"
        style={{ left: `${origin() + props.value * spacing()}px`, color: ruler().chosen(mark()) }}
      >
        {fmt(props.setting.steps[props.value])}
      </span>
      <span
        class="absolute bottom-[3px] h-[20px] w-[3px] -translate-x-1/2 rounded-full transition-[left] duration-150 ease-out"
        style={{ left: `${origin() + props.value * spacing()}px`, background: ruler().chosen(mark()) }}
      />
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
  const controlWidth = () => props.width - ICON_COLUMN - 1 - (d().lines.right ?? 0);
  const pick = (value: number) =>
    props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  return (
    <div
      class="relative flex items-stretch"
      style={{
        height: `${ROW}px`,
        ...d().row(props.focused),
        'padding-top': `${d().lines.top}px`,
        'padding-bottom': `${d().lines.bottom}px`,
        'padding-right': `${d().lines.right ?? 0}px`,
      }}
      onClick={() => props.send({ focus: props.index, side: props.side })}
    >
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <Strip
            items={(props.setting as ChoiceSetting).choices}
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
          <div class="flex flex-1 overflow-hidden" style={d().strip.track(props.focused)}>
            <Show
              when={'kind' in d().slider ? (d().slider as Ruler) : undefined}
              fallback={
                <ThumbSlider
                  setting={slider()}
                  colors={d().slider as ThumbColors}
                  value={props.recipe[slider().id]}
                  design={d()}
                  focused={props.focused}
                  changed={props.changed}
                  pick={pick}
                />
              }
            >
              {(ruler) => (
                <RulerSlider
                  setting={slider()}
                  ruler={ruler()}
                  width={controlWidth()}
                  value={props.recipe[slider().id]}
                  design={d()}
                  focused={props.focused}
                  changed={props.changed}
                  pick={pick}
                />
              )}
            </Show>
          </div>
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
          'mask-image': `linear-gradient(to bottom, ${scroll() > 0 ? 'transparent' : '#000'}, #000 ${FADE_WIDTH}px, #000 calc(100% - ${FADE_WIDTH}px), ${scroll() < SETTINGS.length * ROW - height() ? 'transparent' : '#000'})`,
        }}
      >
        <div style={{ transform: `translateY(${-scroll()}px)` }}>
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
        '--accent': s().mode === 'raw' ? 'var(--color-play)' : ACCENTS[accent()].color,
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
    <div class="flex flex-wrap rounded-[12px] bg-[#1e1e20] p-[3px]">
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
        <Segmented
          choices={[
            { value: 'shadow', label: 'Shadow' },
            { value: 'fade', label: 'Fade' },
            { value: 'clip', label: 'Clip' },
          ]}
          value={depth()}
          pick={setDepth}
        />
        <Segmented
          choices={(Object.keys(ACCENTS) as Accent[]).map((id) => ({
            value: id,
            label: ACCENTS[id].label,
          }))}
          value={accent()}
          pick={setAccent}
        />
        <label class="flex min-h-[44px] items-center gap-[8px] px-[6px] text-[15px] font-semibold">
          <input
            type="checkbox"
            class="size-[20px]"
            checked={center()}
            onChange={(e) => setCenter(e.currentTarget.checked)}
          />
          Center selected
        </label>
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
  grainSize: 1,
  clarity: at(CLARITY, 2),
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
    state: { ...INITIAL, focus: 9, recipe: EDITED },
  },
  { id: 'film-notes', title: 'Film simulation', state: { ...INITIAL, focus: 0 } },
  { id: 'grain-size', title: 'GRAIN SIZE under the grain icon', state: { ...INITIAL, focus: 13, recipe: EDITED } },
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
