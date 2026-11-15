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

/* ---- Look -------------------------------------------------------------------- */

/** Two column treatments to compare: FUJIMOD SET-UP's beveled rows, or the
 * root menu's flat rows. */
type Finish = 'emboss' | 'flat';

interface Skin {
  panel: string;
  /** The column's outer edge. */
  edge: string;
  row: (focused: boolean) => JSX.CSSProperties;
  /** The row's own top and bottom lines, which a drawer's contents stay between. */
  lines: { top: number; bottom: number };
}

const SKINS: Record<Finish, Skin> = {
  emboss: {
    panel: 'rgb(34 34 36 / 0.88)',
    edge: 'inset -1px 0 0 rgb(255 255 255 / 0.3), inset -3px 0 0 #000',
    row: (focused) => ({
      background: focused ? 'rgb(88 88 92 / 0.96)' : undefined,
      'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.2), inset 0 -2px 0 #000',
    }),
    lines: { top: 1, bottom: 2 },
  },
  flat: {
    panel: 'rgb(24 24 24 / 0.88)',
    edge: 'inset -1px 0 0 #3e3e42',
    row: (focused) => ({
      background: focused ? 'rgb(58 58 60 / 0.97)' : undefined,
      'box-shadow': 'inset 0 -1px 0 rgb(255 255 255 / 0.08)',
    }),
    lines: { top: 0, bottom: 1 },
  },
};

/** How a row's control area sits beside its icon: behind a stroke alone, in a
 * darker well, or on a lighter shelf. */
type Drawer = 'line' | 'well' | 'shelf';

const DRAWERS: Record<Drawer, string | undefined> = {
  line: undefined,
  well: 'rgb(0 0 0 / 0.5)',
  shelf: 'rgb(255 255 255 / 0.11)',
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
/** The focus frame around a row's chosen cell or value thumb, and the gap
 * between the frame and the thumb. */
const FRAME = 3;
const FRAME_GAP = 2;
/** From a slider's ends to its first and last steps: half a framed value thumb. */
const INSET = 26;

/** The same scroll for every column, so A and B stay in lockstep. */
function scrollFor(focus: number, height: number): number {
  const max = Math.max(0, SETTINGS.length * ROW - height);
  return Math.max(0, Math.min(max, focus * ROW - (height - ROW) / 2));
}

const CARD = 'rgb(24 24 24 / 0.82)';
const BODY = 'rgb(30 30 32 / 0.96)';
const LIGHT = 'font-light tracking-[0.08em] uppercase';
/** FUJIMOD SET-UP's selection: a white outline just outside the cell. */
const SELECTED = 'outline-[3px] outline-offset-[2px] outline-white';

/* ---- Selection --------------------------------------------------------------- */

/** Treatments to compare for a row's chosen choice and its value thumb. Each
 * fills the drawer's height and shows focus and a change from the compare
 * look its own way. */
type Selection = 'tile' | 'invert' | 'accent' | 'brackets' | 'rules' | 'spotlight';

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

interface ThumbLook {
  /** The full-height box the thumb sits in. */
  box: Look;
  /** The thumb fills its box rather than sitting inside the frame's gap. */
  fill: boolean;
  thumb: string;
}

interface SelectionStyle {
  label: string;
  chosen: (mark: Mark) => Look;
  /** Ink of the choices not chosen, in a focused row or not. */
  other: (focused: boolean) => string;
  thumb: (mark: Mark) => ThumbLook;
}

const ACCENT = 'var(--accent)';
/** The inverted tile and thumb away from focus: opaque, so the track stays behind it. */
const INVERT_REST = '#b4b4b8';
const ring = (width: number, color: string) => `inset 0 0 0 ${width}px ${color}`;
const top = (width: number, color: string) => `inset 0 ${width}px 0 ${color}`;
const bottom = (width: number, color: string) => `inset 0 -${width}px 0 ${color}`;
/** Box shadows listed first paint on top; false entries drop out. */
const edges = (...list: (string | false)[]) =>
  list.filter((edge): edge is string => edge !== false).join(', ') || undefined;

/** The thumb's look when only its number shows a change. */
const plainThumb = (mark: Mark, box: JSX.CSSProperties = {}): ThumbLook => ({
  box: { style: box, ink: mark.changed ? ACCENT : '#000' },
  fill: false,
  thumb: '#fff',
});

const SELECTIONS: Record<Selection, SelectionStyle> = {
  tile: {
    label: 'Tile',
    chosen: (m) => ({
      style: {
        background: 'rgb(255 255 255 / 0.2)',
        'box-shadow': edges(m.changed && bottom(FRAME, ACCENT), m.focused && ring(FRAME, '#fff')),
      },
      ink: '#fff',
    }),
    other: () => '#fff',
    thumb: (m) => plainThumb(m, { 'box-shadow': m.focused ? ring(FRAME, '#fff') : undefined }),
  },
  invert: {
    label: 'Inverted',
    chosen: (m) => ({
      style: { background: m.focused ? '#fff' : INVERT_REST },
      ink: m.changed ? ACCENT : '#000',
    }),
    other: () => '#fff',
    thumb: (m) => ({
      box: { style: {}, ink: m.changed ? ACCENT : '#000' },
      fill: m.focused,
      thumb: m.focused ? '#fff' : INVERT_REST,
    }),
  },
  accent: {
    label: 'Accent',
    chosen: (m) => ({
      style: m.focused
        ? { background: ACCENT }
        : { 'box-shadow': bottom(FRAME, 'rgb(255 255 255 / 0.85)') },
      ink: '#fff',
      pip: m.changed ? (m.focused ? '#fff' : ACCENT) : undefined,
    }),
    other: () => '#fff',
    thumb: (m) =>
      m.focused
        ? {
            box: {
              style: { background: ACCENT },
              ink: '#fff',
              pip: m.changed ? '#fff' : undefined,
            },
            fill: true,
            thumb: ACCENT,
          }
        : plainThumb(m),
  },
  brackets: {
    label: 'Brackets',
    chosen: (m) => ({
      style: {},
      ink: '#fff',
      corners: m.focused
        ? { arm: 12, weight: FRAME, color: '#fff', lower: m.changed ? ACCENT : undefined }
        : {
            arm: 8,
            weight: 2,
            color: 'rgb(255 255 255 / 0.7)',
            lower: m.changed ? ACCENT : undefined,
          },
    }),
    other: () => '#fff',
    thumb: (m) => ({
      ...plainThumb(m),
      box: {
        style: {},
        ink: m.changed ? ACCENT : '#000',
        corners: m.focused ? { arm: 10, weight: FRAME, color: '#fff' } : undefined,
      },
    }),
  },
  rules: {
    label: 'Rules',
    chosen: (m) => {
      const weight = m.focused ? FRAME : 2;
      const rule = m.focused ? '#fff' : 'rgb(255 255 255 / 0.6)';
      return {
        style: {
          'box-shadow': edges(
            top(weight, rule),
            m.changed ? bottom(FRAME, ACCENT) : bottom(weight, rule),
          ),
        },
        ink: '#fff',
      };
    },
    other: () => '#fff',
    thumb: (m) =>
      plainThumb(m, {
        'box-shadow': m.focused ? edges(top(FRAME, '#fff'), bottom(FRAME, '#fff')) : undefined,
      }),
  },
  spotlight: {
    label: 'Spotlight',
    chosen: (m) => ({
      style: {
        background: m.focused ? 'rgb(0 0 0 / 0.72)' : undefined,
        'box-shadow': m.changed ? bottom(FRAME, ACCENT) : undefined,
      },
      ink: '#fff',
    }),
    other: (focused) => (focused ? 'rgb(255 255 255 / 0.6)' : 'rgb(255 255 255 / 0.3)'),
    thumb: (m) => plainThumb(m, { background: m.focused ? 'rgb(0 0 0 / 0.72)' : undefined }),
  },
};

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

/** Stock icons drawn as the Q tiles draw them: the ink in white, its
 * counters cut through to the surface beneath. */
function Ink(props: { src: string; class: string; mode: 'alpha' | 'luminance'; color?: string }) {
  return (
    <span
      class={`shrink-0 bg-white ${props.class}`}
      style={{
        '-webkit-mask': `url(${props.src}) center / contain no-repeat`,
        mask: `url(${props.src}) center / contain no-repeat`,
        'mask-mode': props.mode,
        'background-color': props.color,
      }}
    />
  );
}

function RowIconView(props: { icon: RowIcon }) {
  return (
    <Show
      when={'stock' in props.icon ? props.icon.stock : undefined}
      fallback={
        <Ink
          src={`${MOD}/${(props.icon as { mod: string }).mod}.svg`}
          class="-mx-[2px] h-[30px] w-[44px]"
          mode="alpha"
        />
      }
    >
      {(id) => (
        // Q icons are 80 by 48 with their ink in the middle 52 by 40; at 0.75
        // the ink is the root menu's 39 by 30.
        <Ink src={stock(id())} class="-mx-[10px] h-[36px] w-[60px]" mode="alpha" />
      )}
    </Show>
  );
}

/** Every choice in a row, scrolled to keep the chosen one in view. Text
 * choices sit as close as their words allow. */
function ChoiceStrip(props: {
  setting: ChoiceSetting;
  value: number;
  width: number;
  focused: boolean;
  changed: boolean;
  selection: SelectionStyle;
  pick: (value: number) => void;
}) {
  const look = () => props.selection.chosen({ focused: props.focused, changed: props.changed });
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
            const ink = () => (chosen() ? look().ink : props.selection.other(props.focused));
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
                    <span class={`text-[16px] ${LIGHT}`} style={{ color: ink() }}>
                      {item.choice.name}
                    </span>
                  }
                >
                  {(icon) => (
                    <Ink src={stock(icon())} class="h-[24px] w-[48px]" mode="alpha" color={ink()} />
                  )}
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

/** FUJIMOD SET-UP's segment bar: zero to the value filled, the value itself a
 * wider white thumb that carries the number. */
function Segments(props: {
  setting: SliderSetting;
  value: number;
  focused: boolean;
  changed: boolean;
  selection: SelectionStyle;
  pick: (value: number) => void;
}) {
  const look = () => props.selection.thumb({ focused: props.focused, changed: props.changed });
  const count = () => props.setting.steps.length;
  const zero = () => props.setting.steps.indexOf(0);
  const x = (i: number) => `calc(${INSET}px + (100% - ${2 * INSET}px) * ${i / (count() - 1)})`;
  return (
    <div
      class="relative mx-[6px] flex-1"
      onClick={(e) => {
        e.stopPropagation();
        const box = e.currentTarget.getBoundingClientRect();
        const scale = box.width / e.currentTarget.offsetWidth;
        const inner = (e.clientX - box.left) / scale - INSET;
        const i = Math.round((inner / (e.currentTarget.offsetWidth - 2 * INSET)) * (count() - 1));
        props.pick(Math.max(0, Math.min(count() - 1, i)));
      }}
    >
      <div
        class="absolute top-1/2 h-[2px] -translate-y-1/2 bg-white/35"
        style={{ left: `${INSET}px`, right: `${INSET}px` }}
      />
      <div
        class="absolute top-1/2 h-[4px] -translate-y-1/2 bg-(--accent)"
        style={{
          left: x(Math.min(zero(), props.value)),
          width: `calc((100% - ${2 * INSET}px) * ${Math.abs(props.value - zero()) / (count() - 1)})`,
        }}
      />
      <For each={props.setting.steps.map((step, i) => i)}>
        {(i) => (
          <span
            class="absolute top-1/2 -translate-x-1/2 -translate-y-1/2"
            style={
              i === zero()
                ? { left: x(i), width: '4px', height: '26px', background: '#fff' }
                : {
                    left: x(i),
                    width: '2px',
                    height: '10px',
                    background: 'rgb(255 255 255 / 0.55)',
                  }
            }
          />
        )}
      </For>
      <span
        class="absolute inset-y-0 flex -translate-x-1/2"
        style={{
          left: x(props.value),
          padding: look().fill ? undefined : `${FRAME + FRAME_GAP}px`,
          ...look().box.style,
        }}
      >
        <span
          class="flex items-center justify-center px-[4px]"
          style={{
            'min-width': `${look().fill ? 40 + 2 * (FRAME + FRAME_GAP) : 40}px`,
            background: look().thumb,
          }}
        >
          <span
            class="text-[17px] leading-none font-semibold tabular-nums"
            style={{ color: look().box.ink }}
          >
            {fmt(props.setting.steps[props.value])}
          </span>
        </span>
        <Marks look={look().box} />
      </span>
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
  skin: Skin;
  drawer: Drawer;
  selection: SelectionStyle;
  side?: Side;
  send: Send;
}) {
  const controlWidth = () => props.width - ICON_COLUMN - 1;
  const pick = (value: number) =>
    props.send({ set: props.setting.id, value, focus: props.index, side: props.side });
  return (
    <div
      class="relative flex items-stretch"
      style={{ height: `${ROW}px`, ...props.skin.row(props.focused) }}
      onClick={() => props.send({ focus: props.index, side: props.side })}
    >
      <Show when={props.focused}>
        <span
          class="absolute top-[1px] bottom-[2px] left-0 bg-(--accent)"
          style={{ width: `${STRIPE}px` }}
        />
      </Show>
      <div
        class="flex shrink-0 items-center justify-center"
        style={{ width: `${ICON_COLUMN}px`, 'border-right': '1px solid rgb(255 255 255 / 0.22)' }}
      >
        <RowIconView icon={props.setting.icon} />
      </div>
      <div
        class="flex min-w-0 flex-1 items-stretch"
        style={{
          background: DRAWERS[props.drawer],
          'padding-top': `${props.skin.lines.top}px`,
          'padding-bottom': `${props.skin.lines.bottom}px`,
        }}
      >
        <Show
          when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
          fallback={
            <ChoiceStrip
              setting={props.setting as ChoiceSetting}
              value={props.recipe[props.setting.id]}
              width={controlWidth()}
              focused={props.focused}
              changed={props.changed}
              selection={props.selection}
              pick={pick}
            />
          }
        >
          {(slider) => (
            <Segments
              setting={slider()}
              value={props.recipe[slider().id]}
              focused={props.focused}
              changed={props.changed}
              selection={props.selection}
              pick={pick}
            />
          )}
        </Show>
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
  skin: Skin;
  drawer: Drawer;
  selection: SelectionStyle;
  side?: Side;
  active?: boolean;
  send: Send;
}) {
  const height = () => 480;
  const scroll = () => scrollFor(props.focus, height());
  const edge = () =>
    props.side === 'b' ? props.skin.edge.replaceAll('inset -', 'inset ') : props.skin.edge;
  return (
    <div
      class="absolute inset-y-0 flex flex-col"
      style={{
        left: `${props.left}px`,
        width: `${props.width}px`,
        background: props.skin.panel,
        'box-shadow': edge(),
      }}
    >
      <div
        class="relative flex-1 overflow-hidden"
        style={{
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
                skin={props.skin}
                drawer={props.drawer}
                selection={props.selection}
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
  skin: Skin;
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
        class={`flex h-full items-center justify-between pr-[12px] pl-[10px] text-[19px] ${LIGHT}`}
        style={{ ...props.skin.row(true), 'border-right': '1px solid #3e3e42' }}
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
          background: BODY,
          'box-shadow': 'inset 0 0 0 1px #3e3e42',
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
  finish: Finish;
  drawer: Drawer;
  selection: Selection;
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
  const skin = () => SKINS[s().finish];
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
                  skin={skin()}
                  drawer={s().drawer}
                  selection={SELECTIONS[s().selection]}
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
          skin={skin()}
          drawer={s().drawer}
          selection={SELECTIONS[s().selection]}
          send={send}
        />
        <Show when={s().fly === 'none'}>
          <div
            class="absolute top-[10px] flex flex-col items-start gap-[4px]"
            style={{ left: `${WIDTH + 12}px` }}
          >
            <span
              class="px-[12px] py-[6px] text-[22px] font-light tracking-[0.14em] uppercase"
              style={{ background: CARD }}
            >
              {focused().name}
            </span>
            <span class={`px-[12px] py-[4px] text-[22px] ${LIGHT}`} style={{ background: CARD }}>
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
            skin={skin()}
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
            skin={skin()}
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
  finish: 'emboss',
  drawer: 'shelf',
  selection: 'tile',
};

const PADS = [
  { label: '◀', key: 'ArrowLeft' },
  { label: '▲', key: 'ArrowUp' },
  { label: '▼', key: 'ArrowDown' },
  { label: '▶', key: 'ArrowRight' },
  { label: 'OK', key: 'Enter' },
  { label: 'BACK', key: 'Escape' },
];

const DRAWER_CHOICES: { drawer: Drawer; label: string }[] = [
  { drawer: 'line', label: 'Stroke' },
  { drawer: 'well', label: 'Dark drawer' },
  { drawer: 'shelf', label: 'Light drawer' },
];

const FINISHES: { finish: Finish; label: string }[] = [
  { finish: 'flat', label: 'Flat' },
  { finish: 'emboss', label: 'Embossed' },
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
          choices={FINISHES.map((choice) => ({ value: choice.finish, label: choice.label }))}
          value={state().finish}
          pick={(finish) => setState({ ...state(), finish })}
        />
        <Segmented
          choices={DRAWER_CHOICES.map((choice) => ({ value: choice.drawer, label: choice.label }))}
          value={state().drawer}
          pick={(drawer) => setState({ ...state(), drawer })}
        />
        <Segmented
          choices={(Object.keys(SELECTIONS) as Selection[]).map((selection) => ({
            value: selection,
            label: SELECTIONS[selection].label,
          }))}
          value={state().selection}
          pick={(selection) => setState({ ...state(), selection })}
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

/** Each selection with the WB row focused and changed from A, beside other
 * changed and unchanged rows. */
const SELECTION_STATES = (Object.keys(SELECTIONS) as Selection[]).map((selection) => ({
  id: `selection-${selection}`,
  title: `Selection: ${SELECTIONS[selection].label}`,
  state: {
    ...INITIAL,
    selection,
    snapshot: BASE,
    recipe: { ...EDITED, wb: 1 },
    focus: 1,
  } satisfies EditorState,
}));

const STATES: { id: string; title: string; state: EditorState }[] = [
  ...SELECTION_STATES,
  { id: 'emboss', title: 'Embossed with the light drawer', state: INITIAL },
  {
    id: 'flat',
    title: 'Flat with the stroke',
    state: { ...INITIAL, finish: 'flat', drawer: 'line' },
  },
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
