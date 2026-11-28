import { For, Show, createEffect, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Artwork, Monogram, Wordmark, stockArtwork } from '../../../design/artwork';
import { Disclosure, Row, Scroller, SectionHeader } from '../../../design/column';
import { Check, Chevron, Choice, Dial, signed, type ChoiceItem } from '../../../design/controls';
import {
  BODY,
  CAPTION,
  CHOSEN,
  COLUMN,
  DIM,
  EASE,
  EDITED,
  EXPAND_MS,
  GLASS,
  HAIRLINE,
  HEADER,
  INNER,
  MARGIN,
  RADIUS,
  ROW,
  SELECTED,
  TITLE,
  TRACK,
  VALUE,
  WELL,
} from '../../../design/materials';
import { NumberPad } from '../../../design/numberpad';
import { AnchoredFlyOut, FlyOut, PickerList, flyOutListHeight } from '../../../design/picker';
import { Card, KeyHint } from '../../../design/screen';
import modIcons from '../../../ui/assets/mod-icons.generated.svg?url';
import '../../../design/design.css';

import {
  ColorWheel,
  CurveChannelTabs,
  CurveGraph,
  ExpandIcon,
  NumbersSheet,
  Scope,
  Swatch,
  ToneGraph,
  knotReadout,
} from './film-tools';
import {
  BAND_NAMES,
  BASES,
  CARD_RECIPES,
  CHROMA_SCALES,
  CONTENT,
  FILM_TOOLS,
  FILM_TOOL_NAMES,
  H,
  ICON_WORDS,
  MIX,
  PICTURE_LEFT,
  RAIL,
  RAWS,
  RAW_DEFAULT,
  RAW_OPTIONS,
  RECIPE_SETTINGS,
  SCROLLER,
  SETTING_GROUPS,
  SETTING_GROUP_TITLES,
  SPLIT,
  W,
  activeFilmEntry,
  baseFilm,
  cloneFilm,
  copyGroup,
  enterSection,
  fmtStep,
  groupSource,
  isFilmDirty,
  isFilmToolEdited,
  isInFilmArea,
  isRecipeDirty,
  mixScales,
  nextName,
  openSaveFilmKeyboard,
  padTarget,
  padText,
  press,
  pressPad,
  requestFilmSelect,
  requestReflectShootingCond,
  resetFilmEdits,
  sectionOfFocus,
  settingValueName,
  typing,
  updateCardFilm,
  visibleTopSections,
  type ChoiceSetting,
  type Film,
  type FilmTool,
  type RailId,
  type RecipeSetting,
  type RowIcon,
  type SettingGroupId,
  type SliderSetting,
  type TopSection,
  type UnifiedState,
} from './model';
import {
  FilmSwitchModal,
  Keyboard,
  RAW_STRIP_BAND,
  RawStrip,
  ReflectConfirmModal,
  SHIFT_CAPTION,
  SHIFT_HEIGHT,
  ShiftGrid,
} from './overlays';
import { Picture, useHistogram } from './picture';

/* ---- Constants ------------------------------------------------------------------- */

/** A focused row grows to show its control: a title line and the well. */
const OPEN_ROW = 88;
/** The pictogram before a row's name. */
const PICTOGRAM = { width: 34, height: 26 };

/** A row on the page: flat, the focus plate and stripe when focused, an
 * inset hairline otherwise. */
const CELL = (focused: boolean): JSX.CSSProperties => ({
  ...TRACK(focused),
  'box-shadow': focused ? 'var(--m-focus-edge)' : HAIRLINE,
});

/** A bar of actions: equal cells divided by hairlines. */
const ACTION = (focused: boolean, idle = false): JSX.CSSProperties => ({
  ...TRACK(focused),
  'box-shadow': focused ? 'var(--m-focus-edge)' : 'inset -1px 0 0 var(--m-hairline)',
  color: idle ? 'rgb(255 255 255 / 0.35)' : focused ? '#fff' : DIM,
});

/** The recessed panel the film tools sit in. */
const RECESSED: JSX.CSSProperties = {
  background: 'var(--m-subpanel)',
  'box-shadow': 'var(--m-subpanel-edge)',
};

/** The small dot that marks something changed from what was opened. */
function EditedDot(props: { size?: number }) {
  return (
    <span
      class="shrink-0 rounded-full"
      style={{
        width: `${props.size ?? 8}px`,
        height: `${props.size ?? 8}px`,
        background: EDITED,
      }}
    />
  );
}

/* ---- Rail & row icons ------------------------------------------------------------ */

function RailIcon(props: { id: RailId; color: string; size?: number }) {
  const stroke = () => ({
    fill: 'none',
    stroke: props.color,
    'stroke-width': 2.4,
    'stroke-linecap': 'round' as const,
    'stroke-linejoin': 'round' as const,
  });
  const s = props.size ?? 24;
  return (
    <svg width={s} height={s} viewBox="0 0 34 34">
      <Show when={props.id === 'raw'}>
        <rect x="4" y="8" width="26" height="18" rx="2" {...stroke()} />
        <path d="M14 12.5v9l7.5-4.5z" fill={props.color} />
      </Show>
      <Show when={props.id === 'recipe'}>
        <rect x="6" y="5" width="22" height="24" rx="3" {...stroke()} />
        <path d="M11 12h12M11 17h12M11 22h7" {...stroke()} stroke-width="2.1" />
      </Show>
      <Show when={props.id === 'film'}>
        <rect x="5" y="5" width="24" height="24" rx="2" {...stroke()} />
        <path d="M5 10h24M5 24h24" {...stroke()} stroke-opacity="0.6" />
        <path
          d="M9 7.5h2M15 7.5h2M21 7.5h2M9 26.5h2M15 26.5h2M21 26.5h2"
          {...stroke()}
          stroke-width="2"
        />
      </Show>
      <Show when={props.id === 'curve'}>
        <rect x="5" y="5" width="24" height="24" rx="3" {...stroke()} stroke-opacity="0.5" />
        <path d="M7 27C14 26 13 9 27 7" {...stroke()} />
      </Show>
      <Show when={props.id === 'chroma'}>
        <circle cx="17" cy="17" r="10.5" {...stroke()} />
        <path d="M17 6.5a10.5 10.5 0 0 1 0 21z" fill={props.color} />
      </Show>
      <Show when={props.id === 'mix'}>
        <circle cx="12.5" cy="13.5" r="6" {...stroke()} />
        <circle cx="21.5" cy="13.5" r="6" {...stroke()} />
        <circle cx="17" cy="21.5" r="6" {...stroke()} />
      </Show>
      <Show when={props.id === 'luma'}>
        <path d="M7 24v-4M12 24v-9M17 24v-13M22 24v-7M27 24v-3" {...stroke()} />
        <path d="M5 28h24" {...stroke()} stroke-opacity="0.5" />
      </Show>
      <Show when={props.id === 'light'}>
        <circle cx="17" cy="17" r="6" {...stroke()} />
        <path
          d="M17 4v4M17 26v4M4 17h4M26 17h4M7.8 7.8l2.8 2.8M23.4 23.4l2.8 2.8M26.2 7.8l-2.8 2.8M10.6 23.4l-2.8 2.8"
          {...stroke()}
          stroke-width="2"
        />
      </Show>
      <Show when={props.id === 'color'}>
        <path d="M17 5c5.5 7 9 11 9 15a9 9 0 0 1-18 0c0-4 3.5-8 9-15z" {...stroke()} />
      </Show>
      <Show when={props.id === 'detail'}>
        <path d="M17 5 29 27H5z" {...stroke()} />
        <path d="M17 13v8" {...stroke()} stroke-width="2" />
      </Show>
      <Show when={props.id === 'effects'}>
        <path d="M17 4l2.8 8.2L28 15l-8.2 2.8L17 26l-2.8-8.2L6 15l8.2-2.8z" {...stroke()} />
      </Show>
    </svg>
  );
}

const MOD_ICON_SYMBOLS: Record<string, string> = {
  d_range_priority: 'mod-d-range-priority',
  clarity: 'mod-clarity',
  color_chrome: 'mod-color-chrome-effect',
  color_chrome_blue: 'mod-color-chrome-blue',
  film_grain: 'mod-film-grain-effect-item',
  halation: 'mod-halation-item',
  light_leaks: 'mod-light-leaks-item',
  borders: 'mod-borders-item',
  date_stamp: 'mod-date-stamp-item',
};

function RowIconView(props: { icon: RowIcon; color: string }) {
  const i = () => props.icon;
  const word = (key: string | number, badge?: string) => (
    <Wordmark word={ICON_WORDS[key] ?? String(key)} badge={badge} color={props.color} />
  );
  return (
    <Show
      when={'stock' in i() ? (i() as { stock: number }).stock : undefined}
      fallback={
        'word' in i() ? (
          <Wordmark
            word={(i() as { word: string }).word}
            badge={(i() as { badge?: string }).badge}
            color={props.color}
          />
        ) : (
          (() => {
            const slug = (i() as { mod: string }).mod;
            const sym = MOD_ICON_SYMBOLS[slug];
            return sym ? (
              <svg
                class="h-[26px] w-[39px] shrink-0"
                style={{ color: props.color }}
                aria-hidden="true"
              >
                <use href={`${modIcons}#${sym}`} />
              </svg>
            ) : (
              word(slug)
            );
          })()
        )
      }
    >
      {(stock) => (
        <Artwork
          src={stockArtwork(stock())}
          class="-mx-[4px] h-[28px] w-[48px]"
          color={props.color}
          fallback={word(stock())}
        />
      )}
    </Show>
  );
}

/** A row's pictogram, in a box of one size so names line up. */
function Pictogram(props: { icon?: RowIcon; color: string }) {
  return (
    <span
      class="flex shrink-0 items-center justify-center"
      style={{ width: `${PICTOGRAM.width}px`, height: `${PICTOGRAM.height}px` }}
    >
      <Show when={props.icon}>{(icon) => <RowIconView icon={icon()} color={props.color} />}</Show>
    </span>
  );
}

/* ---- Setting rows (Light / Color / Detail / Effects) ----------------------------- */

/** A setting: one line with its pictogram, name and value; focused, it grows
 * to show its dial or choice strip, the value read in the strip or the
 * dial's pill. */
function SettingRow(props: {
  setting: RecipeSetting;
  s: UnifiedState;
  update: (s: UnifiedState) => void;
}) {
  const focused = () => props.s.focus.kind === 'setting' && props.s.focus.id === props.setting.id;
  const changed = () => props.s.recipe[props.setting.id] !== props.s.savedRecipe[props.setting.id];
  const shifted = () =>
    props.setting.id === 'wb' && (props.s.recipe.shiftR !== 0 || props.s.recipe.shiftB !== 0);
  const focus = () =>
    props.update({
      ...props.s,
      filmOnly: false,
      focus: { kind: 'setting', id: props.setting.id },
      scrollTo: undefined,
    });
  const pick = (value: number) =>
    props.update({
      ...props.s,
      filmOnly: false,
      focus: { kind: 'setting', id: props.setting.id },
      recipe: { ...props.s.recipe, [props.setting.id]: value },
    });
  const name = () =>
    shifted()
      ? `${props.setting.name} ${signed(props.s.recipe.shiftR)},${signed(props.s.recipe.shiftB)}`
      : props.setting.name;

  const items = (): ChoiceItem[] =>
    props.setting.kind === 'choice'
      ? props.setting.choices.map((choice): ChoiceItem => {
          const src = choice.icon === undefined ? undefined : stockArtwork(choice.icon);
          return src === undefined
            ? choice.name
            : {
                name: choice.name,
                art: (ink) => (
                  <Artwork
                    src={src}
                    class="h-[24px] w-[40px]"
                    fillInk={props.setting.id === 'wb'}
                    color={ink}
                  />
                ),
              };
        })
      : [];

  return (
    <div
      data-anchor={`setting-${props.setting.id}`}
      class="relative overflow-hidden"
      style={{
        height: `${focused() ? OPEN_ROW : ROW}px`,
        transition: `height ${EXPAND_MS}ms ${EASE}`,
      }}
    >
      <Show
        when={focused()}
        fallback={
          <button
            type="button"
            class={`flex w-full items-center gap-[8px] text-left whitespace-nowrap ${BODY}`}
            style={{
              height: `${ROW}px`,
              'padding-left': `${MARGIN - 6}px`,
              'padding-right': `${MARGIN}px`,
              color: DIM,
              'box-shadow': HAIRLINE,
            }}
            onClick={focus}
          >
            <Pictogram
              icon={props.setting.continues ? undefined : props.setting.icon}
              color={DIM}
            />
            <span class="min-w-0 grow truncate">{name()}</span>
            <Show when={changed()}>
              <EditedDot />
            </Show>
            <span class={`shrink-0 ${VALUE}`} style={{ color: '#fff' }}>
              {settingValueName(props.setting, props.s.recipe)}
            </span>
          </button>
        }
      >
        <Show
          when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
          fallback={
            <Choice
              name={name()}
              items={items()}
              value={props.s.recipe[props.setting.id] ?? 0}
              width={CONTENT}
              height={OPEN_ROW}
              focused
              changed={changed()}
              pick={pick}
              onFocus={focus}
            />
          }
        >
          {(slider) => (
            <Dial
              name={name()}
              value={props.s.recipe[slider().id] ?? 0}
              min={0}
              max={slider().steps.length - 1}
              step={1}
              zero={Math.max(0, slider().steps.indexOf(0))}
              format={(i) => fmtStep(slider().steps[i])}
              width={CONTENT}
              height={OPEN_ROW}
              focused
              changed={changed()}
              set={pick}
              onFocus={focus}
            />
          )}
        </Show>
      </Show>
    </div>
  );
}

/** What a closed setting group holds: how many of its settings changed. */
function groupSummary(s: UnifiedState, group: SettingGroupId): string {
  const edited = RECIPE_SETTINGS.filter(
    (r) => r.group === group && s.recipe[r.id] !== s.savedRecipe[r.id],
  ).length;
  return edited ? `${edited} EDITED` : '';
}

/** A static section label: the column's sections that do not close. */
function SectionLabel(props: { title: string; trailing?: JSX.Element }) {
  return (
    <div
      class="flex items-center gap-[8px]"
      style={{ height: `${HEADER}px`, 'padding-inline': `${MARGIN}px` }}
    >
      <span class={`shrink-0 grow font-semibold ${CAPTION}`} style={{ color: DIM }}>
        {props.title}
      </span>
      <span class="flex min-w-0 justify-end">{props.trailing}</span>
    </div>
  );
}

/** A film tool's heading: its name, and the chip naming where it comes from
 * (EDITED, or the film it was copied from), which opens the copy list. */
function ToolHeading(props: {
  title: string;
  tool: FilmTool;
  s: UnifiedState;
  update: (s: UnifiedState) => void;
  focused: boolean;
}) {
  const source = () => groupSource(props.s.film, props.tool);
  return (
    <SectionLabel
      title={props.title}
      trailing={
        <button
          type="button"
          data-anchor={`copy-${props.tool}`}
          class={`flex h-[30px] shrink-0 items-center gap-[6px] pr-[8px] pl-[10px] font-medium whitespace-nowrap ${CAPTION}`}
          style={{
            ...TRACK(props.focused),
            background: props.focused ? 'var(--m-focus)' : 'var(--m-chip)',
            'box-shadow': props.focused ? 'var(--m-focus-edge)' : 'var(--m-chip-edge)',
            'border-radius': `${INNER}px`,
            color: '#fff',
          }}
          onClick={() =>
            props.update({
              ...props.s,
              focus: { kind: 'film-tool-header', tool: props.tool },
              picker: { kind: 'copy', tool: props.tool, at: source() ?? 0 },
            })
          }
        >
          <Show when={source() === undefined}>
            <EditedDot size={7} />
          </Show>
          {source() === undefined ? 'EDITED' : BASES[source() as number].name}
          <Chevron dir="down" size={12} color={DIM} />
        </button>
      }
    />
  );
}

/* ---- Main Unified Editor --------------------------------------------------------- */

export function UnifiedEditor(props: { s: UnifiedState; set: (s: UnifiedState) => void }) {
  const s = () => props.s;
  const update = (next: UnifiedState) => props.set(next);
  const histogram = useHistogram();

  let scroller: HTMLDivElement | undefined;
  const sectionRefs: Partial<Record<RailId, HTMLElement>> = {};

  const scrollToSection = (id: RailId, smooth: boolean) => {
    const el = sectionRefs[id];
    if (scroller && el)
      scroller.scrollTo({
        top: Math.max(0, el.offsetTop - 6),
        behavior: smooth ? 'smooth' : 'auto',
      });
  };

  onSettled(() => {
    const target = props.s.scrollTo;
    if (target) scrollToSection(target, false);
  });

  createEffect(
    () => s().scrollTo,
    (target) => {
      if (target) scrollToSection(target, true);
    },
  );

  const toggleSection = (sec: TopSection) => {
    update({
      ...s(),
      collapsed: { ...s().collapsed, [sec]: !s().collapsed[sec] },
    });
  };

  const activeFilm = () => activeFilmEntry(s());
  const dirtyFilm = () => isFilmDirty(s());
  const dirtyRecipe = () => isRecipeDirty(s());

  const compareTarget = ():
    | { film: Film; recipe: UnifiedState['recipe']; split: number }
    | undefined => {
    const c = s().compare;
    if (!c) return undefined;
    if (c.b === 'opened') {
      const savedEntry = s().films.find((f) => f.id === s().savedFilmId) ?? activeFilm();
      return { film: savedEntry.film, recipe: s().savedRecipe, split: c.split };
    }
    return { film: baseFilm(c.b), recipe: s().recipe, split: c.split };
  };

  const focusedSetting = (): RecipeSetting | undefined => {
    const f = s().focus;
    return f.kind === 'setting' ? RECIPE_SETTINGS.find((r) => r.id === f.id) : undefined;
  };

  const hudTitle = (): string | undefined => {
    const setting = focusedSetting();
    if (setting) return setting.name;
    if (isInFilmArea(s().focus)) return 'FILM SIMULATION';
    return undefined;
  };

  const hudValue = (): string | undefined => {
    const setting = focusedSetting();
    if (setting) {
      const base = settingValueName(setting, s().recipe);
      if (setting.id === 'wb' && (s().recipe.shiftR !== 0 || s().recipe.shiftB !== 0)) {
        return `${base} · R${signed(s().recipe.shiftR)} B${signed(s().recipe.shiftB)}`;
      }
      return base;
    }
    if (isInFilmArea(s().focus)) return activeFilm().name;
    return undefined;
  };

  const isRailFocused = (id: RailId) => {
    const f = s().focus;
    return f.kind === 'rail' && f.id === id;
  };
  const isRawFocused = (item: 'strip' | 'reflect' | 'create' | number) => {
    const f = s().focus;
    return f.kind === 'raw' && f.item === item;
  };
  const isRecipeFocused = (item: 'name' | 'save' | 'saveas' | 'open') => {
    const f = s().focus;
    return f.kind === 'recipe' && f.item === item;
  };
  const isFilmActionFocused = (action: 'update' | 'saveas' | 'reset') => {
    const f = s().focus;
    return f.kind === 'film-action' && f.action === action;
  };
  const isToolHeaderFocused = (tool: FilmTool) => {
    const f = s().focus;
    return f.kind === 'film-tool-header' && f.tool === tool;
  };
  const isCurveFocused = () => s().focus.kind === 'film-curve';
  const isLumaFocused = () => s().focus.kind === 'film-luma';
  const isChromaFocused = (row: 0 | 1 | 2 | 3) => {
    const f = s().focus;
    return f.kind === 'film-chroma' && f.row === row;
  };
  const isMixFocused = (row: 'chips' | 0 | 1 | 'numbers') => {
    const f = s().focus;
    return f.kind === 'film-mix' && f.row === row;
  };

  const topSections = () => visibleTopSections(s());
  const railActive = (): RailId => {
    const f = s().focus;
    return f.kind === 'rail' ? f.id : sectionOfFocus(f);
  };

  const hideColumn = () => s().fullCurve || Boolean(s().keyboard);

  return (
    <div
      class="absolute inset-0"
      style={{ '--m-accent': s().mode === 'raw' ? 'var(--color-play)' : undefined }}
    >
      {/* Live developed picture */}
      <Picture
        film={s().film}
        recipe={s().recipe}
        filmOnly={s().filmOnly}
        crop={s().mode === 'raw' ? s().crop : 0}
        compare={compareTarget()}
      />

      {/* Compare A | B split divider & pills */}
      <Show when={s().compare}>
        {(c) => (
          <>
            <div
              class="absolute inset-y-0 touch-none"
              style={{ left: `${PICTURE_LEFT}px`, right: '0' }}
              onPointerDown={(e) => e.currentTarget.setPointerCapture(e.pointerId)}
              onPointerMove={(e) => {
                if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
                const box = e.currentTarget.getBoundingClientRect();
                const x = PICTURE_LEFT + ((e.clientX - box.left) / box.width) * (W - PICTURE_LEFT);
                update({
                  ...s(),
                  compare: {
                    ...c(),
                    split: Math.round(Math.max(PICTURE_LEFT + 28, Math.min(W - 28, x))),
                  },
                });
              }}
            />
            <div
              class="pointer-events-none absolute inset-y-0 w-[3px] -translate-x-1/2 bg-white"
              style={{ left: `${c().split}px`, 'box-shadow': '0 0 6px rgb(0 0 0 / 0.6)' }}
            />
            <div
              class="pointer-events-none absolute top-[220px] flex size-[38px] -translate-x-1/2 items-center justify-center rounded-full bg-white text-[20px] font-semibold text-black"
              style={{ left: `${c().split}px` }}
            >
              ⇆
            </div>
            <div
              class="absolute"
              style={{
                left: `${PICTURE_LEFT + 8}px`,
                top: `${s().mode === 'raw' ? RAW_STRIP_BAND + 6 : 10}px`,
              }}
            >
              <KeyHint glyph="A" label={s().slot} />
            </div>
            <div
              class="absolute right-[10px]"
              data-anchor="compare"
              style={{ top: `${s().mode === 'raw' ? RAW_STRIP_BAND + 6 : 10}px` }}
            >
              <KeyHint
                glyph="B"
                label={c().b === 'opened' ? 'SAVED' : BASES[c().b as number].name}
                tap={() =>
                  update({
                    ...s(),
                    picker: { kind: 'compare', at: c().b === 'opened' ? 0 : (c().b as number) + 1 },
                  })
                }
              />
            </div>
          </>
        )}
      </Show>

      {/* RAW frame strip over top of right picture */}
      <Show when={s().mode === 'raw' && !s().picker && !s().wbShift.open}>
        <RawStrip s={s()} update={update} />
      </Show>

      {/* Left 60px quick-scroll rail + 392px glass scroller */}
      <div
        class="absolute inset-y-0 left-0"
        style={{
          width: `${RAIL + SCROLLER}px`,
          ...(hideColumn() ? { opacity: 0, 'pointer-events': 'none' } : {}),
          transition: 'opacity 180ms ease-out',
        }}
      >
        {/* The rail: one button per section, the section index */}
        <div
          class="absolute inset-y-0 left-0 flex flex-col items-center gap-[4px] overflow-y-auto py-[8px]"
          style={{ ...COLUMN, width: `${RAIL}px`, 'scrollbar-width': 'none' }}
        >
          <For each={topSections()}>
            {(sec) => {
              const dimmed = () => s().filmOnly && SETTING_GROUPS.includes(sec as SettingGroupId);
              const edited = () =>
                sec === 'film'
                  ? dirtyFilm() || s().filmId !== s().savedFilmId
                  : sec === 'recipe'
                    ? false
                    : sec === 'raw'
                      ? s().raw.some((v, k) => v !== RAW_DEFAULT[k])
                      : groupSummary(s(), sec as SettingGroupId) !== '';
              return (
                <button
                  type="button"
                  aria-label={sec}
                  class="relative flex size-[40px] shrink-0 items-center justify-center"
                  style={{
                    ...(isRailFocused(sec)
                      ? TRACK(true)
                      : railActive() === sec
                        ? SELECTED
                        : TRACK(false)),
                    'border-radius': `${INNER}px`,
                    opacity: dimmed() ? 0.42 : 1,
                  }}
                  onClick={() => {
                    scrollToSection(sec, true);
                    update(enterSection(s(), sec));
                  }}
                >
                  <RailIcon
                    id={sec}
                    color={isRailFocused(sec) || railActive() === sec ? '#fff' : DIM}
                    size={24}
                  />
                  <Show when={edited()}>
                    <span class="absolute top-[4px] right-[4px] flex">
                      <EditedDot size={6} />
                    </span>
                  </Show>
                </button>
              );
            }}
          </For>
        </div>

        {/* The film tools' number sheet, over the column */}
        <Show when={s().numbers}>
          <NumbersSheet s={s()} update={update} />
        </Show>

        <Scroller
          ref={(element) => (scroller = element)}
          class="absolute inset-y-0"
          style={{
            left: `${RAIL}px`,
            width: `${SCROLLER}px`,
            'padding-inline': `${(SCROLLER - CONTENT) / 2}px`,
            visibility: s().numbers ? 'hidden' : undefined,
          }}
        >
          {/* RAW CONVERSION */}
          <Show when={s().mode === 'raw'}>
            <section ref={(el) => (sectionRefs.raw = el)}>
              <SectionHeader
                title="RAW CONVERSION"
                summary={`${s().crop}/${RAWS.length}`}
                open={!s().collapsed.raw}
                onToggle={() => toggleSection('raw')}
              />
              <Disclosure open={!s().collapsed.raw}>
                <button
                  type="button"
                  class="flex w-full items-center gap-[8px] text-left"
                  style={{
                    height: `${OPEN_ROW - 24}px`,
                    'padding-inline': `${MARGIN}px`,
                    ...CELL(isRawFocused('strip')),
                  }}
                  onClick={() => update({ ...s(), focus: { kind: 'raw', item: 'strip' } })}
                >
                  <span class="flex min-w-0 grow flex-col gap-[6px]">
                    <span class={`truncate font-semibold text-white ${BODY}`}>
                      {RAWS[s().crop - 1].name}
                    </span>
                    <span class={`truncate tabular-nums ${CAPTION}`} style={{ color: DIM }}>
                      {RAWS[s().crop - 1].info}
                    </span>
                  </span>
                  <span class={`shrink-0 ${VALUE}`} style={{ color: DIM }}>
                    {s().crop}/{RAWS.length}
                  </span>
                </button>
                <For each={RAW_OPTIONS}>
                  {(o, k) => (
                    <Row
                      anchor={`raw-${k()}`}
                      name={
                        <span class="flex items-center gap-[8px]">
                          {o.short}
                          <Show when={(s().raw[k()] ?? 0) !== RAW_DEFAULT[k()]}>
                            <EditedDot />
                          </Show>
                        </span>
                      }
                      value={o.values[s().raw[k()] ?? 0]}
                      focused={isRawFocused(k())}
                      more
                      onClick={() =>
                        update({
                          ...s(),
                          focus: { kind: 'raw', item: k() },
                          picker: { kind: 'raw-option', index: k(), at: s().raw[k()] ?? 0 },
                        })
                      }
                    />
                  )}
                </For>
                <div class="flex" style={{ height: `${ROW}px`, 'box-shadow': HAIRLINE }}>
                  <button
                    type="button"
                    class={`flex min-w-0 grow basis-0 items-center justify-center font-semibold whitespace-nowrap ${CAPTION}`}
                    style={ACTION(isRawFocused('reflect'))}
                    onClick={() =>
                      update(
                        requestReflectShootingCond({
                          ...s(),
                          focus: { kind: 'raw', item: 'reflect' },
                        }),
                      )
                    }
                  >
                    REFLECT COND.
                  </button>
                  <button
                    type="button"
                    class={`flex min-w-0 grow basis-0 items-center justify-center gap-[8px] font-semibold whitespace-nowrap ${CAPTION}`}
                    style={ACTION(isRawFocused('create'))}
                    onClick={() =>
                      update({ ...s(), created: s().crop, focus: { kind: 'raw', item: 'create' } })
                    }
                  >
                    {s().created === s().crop ? 'CREATED' : 'CREATE JPEG'}
                    <Show when={s().created === s().crop}>
                      <Check />
                    </Show>
                  </button>
                </div>
              </Disclosure>
            </section>
          </Show>

          {/* RECIPE */}
          <section ref={(el) => (sectionRefs.recipe = el)}>
            <SectionHeader
              title="RECIPE"
              summary={s().slot}
              open={!s().collapsed.recipe}
              onToggle={() => toggleSection('recipe')}
            />
            <Disclosure open={!s().collapsed.recipe}>
              <button
                type="button"
                class="flex w-full items-center gap-[12px] text-left"
                style={{
                  height: `${OPEN_ROW - 16}px`,
                  'padding-inline': `${MARGIN}px`,
                  ...CELL(isRecipeFocused('name')),
                }}
                onClick={() =>
                  update({
                    ...s(),
                    focus: { kind: 'recipe', item: 'name' },
                    keyboard: typing(s().recipeName, 'recipe-name'),
                  })
                }
              >
                <span
                  class={`flex h-[32px] min-w-[36px] shrink-0 items-center justify-center px-[6px] ${VALUE}`}
                  style={{ ...CHOSEN, 'border-radius': `${INNER}px` }}
                >
                  {s().slot}
                </span>
                <span class="flex min-w-0 grow flex-col gap-[6px]">
                  <span class={`truncate text-white ${TITLE}`}>{s().recipeName}</span>
                  <span
                    class={`flex items-center gap-[6px] truncate ${CAPTION}`}
                    style={{ color: DIM }}
                  >
                    {s().recipeAuthor}
                    <Show when={dirtyRecipe()}>
                      <EditedDot size={6} />
                      <span style={{ color: EDITED }}>EDITED</span>
                    </Show>
                  </span>
                </span>
              </button>
              <div class="flex" style={{ height: `${ROW}px`, 'box-shadow': HAIRLINE }}>
                <button
                  type="button"
                  class={`flex min-w-0 grow basis-0 items-center justify-center font-semibold whitespace-nowrap ${CAPTION}`}
                  style={ACTION(isRecipeFocused('save'), !dirtyRecipe())}
                  onClick={() =>
                    update({
                      ...s(),
                      savedRecipe: { ...s().recipe },
                      savedFilmId: s().filmId,
                      focus: { kind: 'recipe', item: 'save' },
                    })
                  }
                >
                  SAVE
                </button>
                <button
                  type="button"
                  class={`flex min-w-0 grow-[1.6] basis-0 items-center justify-center font-semibold whitespace-nowrap ${CAPTION}`}
                  style={ACTION(isRecipeFocused('saveas'))}
                  onClick={() =>
                    update({
                      ...s(),
                      focus: { kind: 'recipe', item: 'saveas' },
                      keyboard: typing(nextName(s().recipeName), 'recipe-saveas'),
                    })
                  }
                >
                  SAVE AS NEW
                </button>
                <button
                  type="button"
                  data-anchor="open-recipe"
                  class={`flex min-w-0 grow basis-0 items-center justify-center font-semibold whitespace-nowrap ${CAPTION}`}
                  style={ACTION(isRecipeFocused('open'))}
                  onClick={() =>
                    update({
                      ...s(),
                      focus: { kind: 'recipe', item: 'open' },
                      picker: { kind: 'open-recipe', at: 0 },
                    })
                  }
                >
                  OPEN
                </button>
              </div>
            </Disclosure>
          </section>

          {/* FILM SIMULATION: the film strip, then its tools in a recessed panel */}
          <section ref={(el) => (sectionRefs.film = el)}>
            <SectionLabel
              title="FILM SIMULATION"
              trailing={
                <span class={`truncate font-semibold text-white ${CAPTION}`}>
                  {activeFilm().name}
                </span>
              }
            />
            <div
              style={{
                height: `${ROW + 8}px`,
                'padding-inline': `${MARGIN}px`,
                'padding-bottom': '8px',
                'box-shadow': HAIRLINE,
              }}
            >
              <Choice
                items={s().films.map((entry): ChoiceItem => {
                  const src =
                    entry.stockIcon !== undefined ? stockArtwork(entry.stockIcon) : undefined;
                  const label = entry.icon.startsWith('text:')
                    ? entry.icon.slice(5)
                    : entry.short.replace(/\s+/g, '').slice(0, 3).toUpperCase();
                  return {
                    name: entry.name,
                    art: (ink) =>
                      src !== undefined ? (
                        <Artwork
                          src={src}
                          class="h-[24px] w-[40px]"
                          color={ink}
                          fallback={<Monogram text={label} size={26} color={ink} />}
                        />
                      ) : (
                        <Monogram text={label} size={26} color={ink} />
                      ),
                  };
                })}
                value={Math.max(
                  0,
                  s().films.findIndex((e) => e.id === s().filmId),
                )}
                width={CONTENT - 2 * MARGIN}
                height={ROW}
                focused={s().focus.kind === 'film-tiles'}
                changed={s().filmId !== s().savedFilmId}
                pick={(idx) => {
                  const target = s().films[idx];
                  if (target)
                    update(requestFilmSelect({ ...s(), focus: { kind: 'film-tiles' } }, target.id));
                }}
                onFocus={() => update({ ...s(), focus: { kind: 'film-tiles' } })}
              />
            </div>
            <SectionHeader
              title="FILM TOOLS"
              summary={
                FILM_TOOLS.filter((tool) => isFilmToolEdited(s(), tool)).length
                  ? `${FILM_TOOLS.filter((tool) => isFilmToolEdited(s(), tool)).length} EDITED`
                  : ''
              }
              open={!s().collapsed.film}
              focused={s().focus.kind === 'film-header'}
              onToggle={() =>
                update({
                  ...s(),
                  focus: { kind: 'film-header' },
                  collapsed: { ...s().collapsed, film: !s().collapsed.film },
                })
              }
            />
            <Disclosure open={!s().collapsed.film}>
              <div style={RECESSED}>
                {/* The edited film's file actions */}
                <Show when={dirtyFilm()}>
                  <div class="flex" style={{ height: `${ROW}px`, 'box-shadow': HAIRLINE }}>
                    <Show when={activeFilm().isCard}>
                      <button
                        type="button"
                        class={`flex min-w-0 grow basis-0 items-center justify-center px-[6px] font-semibold whitespace-nowrap ${CAPTION}`}
                        style={ACTION(isFilmActionFocused('update'))}
                        onClick={() => update(updateCardFilm(s()))}
                      >
                        UPDATE
                      </button>
                    </Show>
                    <button
                      type="button"
                      class={`flex min-w-0 grow-[1.6] basis-0 items-center justify-center px-[6px] font-semibold whitespace-nowrap ${CAPTION}`}
                      style={ACTION(isFilmActionFocused('saveas'))}
                      onClick={() => update(openSaveFilmKeyboard(s()))}
                    >
                      SAVE AS NEW
                    </button>
                    <button
                      type="button"
                      class={`flex min-w-0 grow basis-0 items-center justify-center px-[6px] font-semibold whitespace-nowrap ${CAPTION}`}
                      style={ACTION(isFilmActionFocused('reset'))}
                      onClick={() => update(resetFilmEdits(s()))}
                    >
                      RESET
                    </button>
                  </div>
                </Show>

                {/* TONE CURVE */}
                <div ref={(el) => (sectionRefs.curve = el)} class="pb-[8px]">
                  <ToolHeading
                    title="TONE CURVE"
                    tool="curve"
                    s={s()}
                    update={update}
                    focused={isToolHeaderFocused('curve')}
                  />
                  <div
                    class="relative mx-[8px] p-[4px]"
                    style={{
                      ...TRACK(isCurveFocused()),
                      ...(isCurveFocused() ? {} : WELL),
                      'border-radius': `${RADIUS}px`,
                    }}
                    onClick={() => update({ ...s(), focus: { kind: 'film-curve' } })}
                  >
                    <div class="flex items-center justify-between px-[4px] py-[2px]">
                      <button
                        type="button"
                        class="flex size-[28px] items-center justify-center"
                        style={{ background: 'var(--m-chip)', 'border-radius': `${INNER - 2}px` }}
                        onPointerDown={(e) => e.stopPropagation()}
                        onClick={() =>
                          update({
                            ...s(),
                            focus: { kind: 'film-curve' },
                            inside: true,
                            fullCurve: true,
                          })
                        }
                      >
                        <ExpandIcon />
                      </button>
                      <CurveChannelTabs
                        channel={s().curveChannel}
                        film={s().film}
                        onSelect={(curveChannel) =>
                          update({ ...s(), curveChannel, focus: { kind: 'film-curve' } })
                        }
                      />
                    </div>
                    <CurveGraph
                      film={s().film}
                      opened={activeFilm().film}
                      channel={s().curveChannel}
                      histogram={histogram()}
                      width={CONTENT - 24}
                      height={148}
                      focus={s().inside && isCurveFocused() ? s().knot : -1}
                      bend={s().inside && isCurveFocused() && s().bend}
                      set={(film) => update({ ...s(), film })}
                      focusKnot={(knot) =>
                        update({ ...s(), knot, focus: { kind: 'film-curve' }, inside: true })
                      }
                      active={s().inside && isCurveFocused()}
                    />
                    <div
                      class={`flex h-[24px] items-center px-[6px] tabular-nums ${CAPTION}`}
                      style={{ color: '#fff' }}
                    >
                      {s().inside && isCurveFocused()
                        ? knotReadout(s().film, s().curveChannel, s().knot, s().bend)
                        : ''}
                    </div>
                  </div>
                </div>

                {/* CHROMA */}
                <div ref={(el) => (sectionRefs.chroma = el)} class="pb-[8px]">
                  <ToolHeading
                    title="CHROMA"
                    tool="chroma"
                    s={s()}
                    update={update}
                    focused={isToolHeaderFocused('chroma')}
                  />
                  <ColorWheel film={s().film} set={(film) => update({ ...s(), film })} />
                  <For each={CHROMA_SCALES}>
                    {(c, k) => (
                      <Dial
                        name={c.name}
                        value={c.get(s().film)}
                        min={c.min}
                        max={c.max}
                        step={c.step}
                        width={CONTENT}
                        focused={isChromaFocused(k() as 0 | 1 | 2)}
                        changed={c.get(s().film) !== c.get(activeFilm().film)}
                        track={c.track}
                        onFocus={() =>
                          update({
                            ...s(),
                            focus: { kind: 'film-chroma', row: k() as 0 | 1 | 2 },
                            inside: false,
                          })
                        }
                        set={(v) =>
                          update({
                            ...s(),
                            focus: { kind: 'film-chroma', row: k() as 0 | 1 | 2 },
                            film: c.put(s().film, v),
                          })
                        }
                      />
                    )}
                  </For>
                  <Row
                    name="NUMBERS"
                    value="2×2"
                    more
                    focused={isChromaFocused(3)}
                    onClick={() =>
                      update({
                        ...s(),
                        focus: { kind: 'film-chroma', row: 3 },
                        numbers: { tool: 'chroma', cell: 0 },
                      })
                    }
                  />
                </div>

                {/* COLOR MIX */}
                <div ref={(el) => (sectionRefs.mix = el)} class="pb-[8px]">
                  <ToolHeading
                    title="COLOR MIX"
                    tool="mix"
                    s={s()}
                    update={update}
                    focused={isToolHeaderFocused('mix')}
                  />
                  <Scope
                    film={s().film}
                    crop={s().mode === 'raw' ? s().crop : 0}
                    chosen={s().primary}
                    choose={(primary) =>
                      update({ ...s(), primary, focus: { kind: 'film-mix', row: 0 } })
                    }
                    set={(film) => update({ ...s(), film })}
                  />
                  <div
                    class="mx-[8px] my-[4px] flex gap-[4px] p-[4px]"
                    style={{
                      ...TRACK(isMixFocused('chips')),
                      ...(isMixFocused('chips') ? {} : WELL),
                      'border-radius': `${INNER + 2}px`,
                    }}
                    onPointerDown={() =>
                      update({ ...s(), focus: { kind: 'film-mix', row: 'chips' } })
                    }
                  >
                    <For each={MIX}>
                      {(mix, p) => (
                        <button
                          type="button"
                          class={`flex h-[36px] flex-1 items-center justify-center gap-[6px] font-medium ${CAPTION}`}
                          style={{
                            ...(s().primary === p() ? CHOSEN : { color: DIM }),
                            'border-radius': `${INNER}px`,
                          }}
                          onClick={() =>
                            update({
                              ...s(),
                              primary: p(),
                              focus: { kind: 'film-mix', row: 'chips' },
                            })
                          }
                        >
                          <Swatch film={s().film} mix={p()} />
                          {mix.name}
                        </button>
                      )}
                    </For>
                  </div>
                  <For each={[0, 1] as const}>
                    {(k) => {
                      const c = () => mixScales(s().primary)[k];
                      return (
                        <Dial
                          name={c().name}
                          value={c().get(s().film)}
                          min={c().min}
                          max={c().max}
                          step={c().step}
                          width={CONTENT}
                          focused={isMixFocused(k)}
                          changed={c().get(s().film) !== c().get(activeFilm().film)}
                          track={c().track}
                          onFocus={() =>
                            update({ ...s(), focus: { kind: 'film-mix', row: k }, inside: false })
                          }
                          set={(v) =>
                            update({
                              ...s(),
                              focus: { kind: 'film-mix', row: k },
                              film: c().put(s().film, v),
                            })
                          }
                        />
                      );
                    }}
                  </For>
                  <Row
                    name="NUMBERS"
                    value="3×3"
                    more
                    focused={isMixFocused('numbers')}
                    onClick={() =>
                      update({
                        ...s(),
                        focus: { kind: 'film-mix', row: 'numbers' },
                        numbers: { tool: 'mix', cell: 1 },
                      })
                    }
                  />
                </div>

                {/* LUMA SAT */}
                <div ref={(el) => (sectionRefs.luma = el)} class="pb-[8px]">
                  <ToolHeading
                    title="LUMA SAT"
                    tool="luma"
                    s={s()}
                    update={update}
                    focused={isToolHeaderFocused('luma')}
                  />
                  <div
                    class="mx-[8px] p-[6px]"
                    style={{
                      ...TRACK(isLumaFocused()),
                      ...(isLumaFocused() ? {} : WELL),
                      'border-radius': `${RADIUS}px`,
                    }}
                    onClick={() => update({ ...s(), focus: { kind: 'film-luma' } })}
                  >
                    <ToneGraph
                      film={s().film}
                      opened={activeFilm().film}
                      width={CONTENT - 28}
                      height={132}
                      band={s().band}
                      inside={s().inside && isLumaFocused()}
                      set={(film) => update({ ...s(), film })}
                      focusBand={(band) =>
                        update({ ...s(), band, focus: { kind: 'film-luma' }, inside: true })
                      }
                    />
                    <div
                      class={`flex h-[24px] items-center px-[4px] tabular-nums ${CAPTION}`}
                      style={{ color: '#fff' }}
                    >
                      {s().inside && isLumaFocused()
                        ? `${BAND_NAMES[s().band]} · ${signed(s().film.bands[s().band])}%`
                        : ''}
                    </div>
                  </div>
                </div>
              </div>
            </Disclosure>
          </section>

          {/* LIGHT, COLOR, DETAIL, EFFECTS */}
          <For each={SETTING_GROUPS}>
            {(group) => (
              <section
                ref={(el) => (sectionRefs[group] = el)}
                style={{
                  opacity: s().filmOnly ? 0.42 : 1,
                  transition: `opacity ${EXPAND_MS}ms ${EASE}`,
                }}
              >
                <SectionHeader
                  title={SETTING_GROUP_TITLES[group]}
                  summary={groupSummary(s(), group)}
                  open={!s().collapsed[group]}
                  onToggle={() => toggleSection(group)}
                />
                <Disclosure open={!s().collapsed[group]}>
                  <For each={RECIPE_SETTINGS.filter((r) => r.group === group)}>
                    {(setting) => <SettingRow setting={setting} s={s()} update={update} />}
                  </For>
                </Disclosure>
              </section>
            )}
          </For>

          <div class="h-[180px]" />
        </Scroller>
      </div>

      {/* Floating HUD over the right live-view area */}
      <Show
        when={
          !s().compare &&
          !s().wbShift.open &&
          !s().picker &&
          !s().fullCurve &&
          !s().keyboard &&
          !s().pad &&
          !s().switchModal &&
          !s().reflectModal
        }
      >
        <div
          class="absolute flex flex-col items-start gap-[5px]"
          style={{
            left: `${PICTURE_LEFT + 10}px`,
            top: `${s().mode === 'raw' ? RAW_STRIP_BAND + 6 : 10}px`,
            'max-width': `${W - PICTURE_LEFT - 18}px`,
          }}
        >
          <Show when={hudTitle()}>
            {(t) => (
              <Card class={`px-[12px] py-[8px] ${CAPTION}`} style={{ color: DIM }}>
                {t()}
              </Card>
            )}
          </Show>
          <Show when={hudValue()}>
            {(v) => (
              <Card class={`flex items-center gap-[8px] px-[12px] py-[8px] ${VALUE}`}>
                <Show when={isInFilmArea(s().focus) && dirtyFilm()}>
                  <EditedDot />
                </Show>
                <span>{v()}</span>
              </Card>
            )}
          </Show>
          <Show when={focusedSetting()?.id === 'wb'}>
            <KeyHint
              glyph="Fn"
              label="WB SHIFT"
              tap={() =>
                update({
                  ...s(),
                  filmOnly: false,
                  wbShift: { open: true, from: [s().recipe.shiftR, s().recipe.shiftB] },
                })
              }
            />
          </Show>
          <Show when={isInFilmArea(s().focus) || s().filmOnly}>
            <KeyHint
              glyph="Fn"
              label={s().filmOnly ? 'FILM ONLY ON' : 'FILM ONLY'}
              on={s().filmOnly}
              tap={() => update({ ...s(), filmOnly: !s().filmOnly })}
            />
          </Show>
        </div>
      </Show>

      {/* Bottom-right key hints */}
      <Show
        when={
          !s().wbShift.open &&
          !s().picker &&
          !s().keyboard &&
          !s().pad &&
          !s().switchModal &&
          !s().reflectModal
        }
      >
        <div class="absolute right-[10px] bottom-[10px] flex flex-col items-end gap-[6px]">
          <Show when={s().inside && isCurveFocused()}>
            <KeyHint
              glyph="OK"
              label={s().fullCurve ? 'SIDEBAR' : 'FULL SCREEN'}
              tap={() => update(press(s(), 'ok'))}
            />
          </Show>
          <Show when={!s().fullCurve}>
            <div class="flex gap-[6px]">
              <KeyHint
                glyph="Q"
                label="A|B"
                on={Boolean(s().compare)}
                tap={() => update(press(s(), 'q'))}
              />
              <KeyHint
                glyph="▶"
                label={s().mode === 'raw' ? 'LIVE' : 'RAW'}
                on={s().mode === 'raw'}
                tap={() => update(press(s(), 'play'))}
              />
            </div>
          </Show>
        </div>
      </Show>

      {/* WB SHIFT 2D flyout */}
      <Show when={s().wbShift.open}>
        <FlyOut
          left={PICTURE_LEFT}
          rowTop={84}
          title="WB SHIFT"
          height={SHIFT_HEIGHT + SHIFT_CAPTION + 22}
        >
          <div class="flex flex-col items-center px-[12px] pt-[12px] pb-[2px]">
            <ShiftGrid
              r={s().recipe.shiftR}
              b={s().recipe.shiftB}
              onShift={(shiftR, shiftB) =>
                update({ ...s(), recipe: { ...s().recipe, shiftR, shiftB } })
              }
            />
            <span class={`flex h-[30px] items-center gap-[16px] ${VALUE}`}>
              <span>R {signed(s().recipe.shiftR)}</span>
              <span>B {signed(s().recipe.shiftB)}</span>
            </span>
          </div>
        </FlyOut>
      </Show>

      {/* Full-screen Tone Curve */}
      <Show when={s().fullCurve}>
        <div class="absolute inset-0 z-20" style={{ background: 'rgb(0 0 0 / 0.42)' }}>
          <div class="absolute top-[14px] left-[24px]">
            <CurveGraph
              film={s().film}
              opened={activeFilm().film}
              channel={s().curveChannel}
              histogram={histogram()}
              width={W - 48}
              height={H - 80}
              focus={s().knot}
              bend={s().bend}
              set={(film) => update({ ...s(), film })}
              focusKnot={(knot) => update({ ...s(), knot })}
              active
            />
          </div>
          <div class="absolute top-[18px] left-[28px] flex items-center gap-[10px]">
            <button
              type="button"
              class="flex size-[40px] items-center justify-center"
              style={{ ...GLASS, 'border-radius': `${INNER + 2}px` }}
              onClick={() => update({ ...s(), fullCurve: false })}
            >
              <ExpandIcon full />
            </button>
            <CurveChannelTabs
              channel={s().curveChannel}
              film={s().film}
              onSelect={(curveChannel) => update({ ...s(), curveChannel })}
            />
          </div>
          <div
            class={`absolute bottom-[16px] left-[24px] flex h-[36px] items-center px-[12px] tabular-nums ${CAPTION}`}
            style={{ ...GLASS, 'border-radius': `${INNER + 2}px` }}
          >
            {knotReadout(s().film, s().curveChannel, s().knot, s().bend)}
          </div>
        </div>
      </Show>

      {/* NumberPad */}
      <Show when={s().pad}>
        {(pad) => {
          const target = () => padTarget(s(), pad());
          return (
            <NumberPad
              name={target().name}
              text={pad().text}
              range={`${padText(target().min, target().decimals)} to ${padText(target().max, target().decimals)}`}
              decimals={target().decimals}
              at={pad().at}
              press={(key, at) => update(pressPad(s(), pad(), { press: key, at }))}
            />
          );
        }}
      </Show>

      {/* Pickers (copy-source, open recipe, compare B, RAW option, setting list) */}
      <Show when={s().picker}>
        {(picker) => {
          const p = () => picker();
          const anchor = () => {
            const q = p();
            if (q.kind === 'copy') return `copy-${q.tool}`;
            if (q.kind === 'open-recipe') return 'open-recipe';
            if (q.kind === 'compare') return 'compare';
            if (q.kind === 'raw-option') return `raw-${q.index}`;
            return `setting-${q.settingId}`;
          };
          const options = () => {
            const q = p();
            if (q.kind === 'copy') return BASES.map((b) => ({ label: b.name }));
            if (q.kind === 'open-recipe')
              return CARD_RECIPES.map((r) => ({ label: `${r.slot} ${r.name}` }));
            if (q.kind === 'compare')
              return [{ label: 'SAVED' }, ...BASES.map((b) => ({ label: b.name }))];
            if (q.kind === 'raw-option')
              return (RAW_OPTIONS[q.index]?.values ?? []).map((v) => ({ label: v }));
            const setting = RECIPE_SETTINGS.find((r) => r.id === q.settingId) as
              | ChoiceSetting
              | undefined;
            return (setting?.choices ?? []).map((choice, idx) => {
              const src = choice.icon === undefined ? undefined : stockArtwork(choice.icon);
              return {
                label: choice.name,
                art: src
                  ? (ink: string) => (
                      <Artwork
                        src={src}
                        class="h-[28px] w-[46px]"
                        fillInk={q.settingId === 'wb'}
                        color={ink}
                      />
                    )
                  : undefined,
                more:
                  q.settingId === 'wb'
                    ? () =>
                        update({
                          ...s(),
                          picker: null,
                          filmOnly: false,
                          recipe: { ...s().recipe, wb: idx },
                          wbShift: { open: true, from: [s().recipe.shiftR, s().recipe.shiftB] },
                        })
                    : undefined,
              };
            });
          };
          const title = () => {
            const q = p();
            if (q.kind === 'copy') return `COPY ${FILM_TOOL_NAMES[q.tool]} FROM`;
            if (q.kind === 'open-recipe') return 'OPEN RECIPE';
            if (q.kind === 'compare') return 'COMPARE WITH';
            if (q.kind === 'raw-option') return RAW_OPTIONS[q.index]?.short;
            return RECIPE_SETTINGS.find((r) => r.id === q.settingId)?.name;
          };
          const currentIdx = () => {
            const q = p();
            if (q.kind === 'copy') return groupSource(s().film, q.tool);
            if (q.kind === 'compare') {
              const b = s().compare?.b;
              return b === 'opened' ? 0 : typeof b === 'number' ? b + 1 : undefined;
            }
            if (q.kind === 'raw-option') return s().raw[q.index];
            if (q.kind === 'setting-list') return s().recipe[q.settingId];
            return undefined;
          };
          return (
            <AnchoredFlyOut
              anchor={anchor()}
              left={PICTURE_LEFT}
              title={title()}
              height={flyOutListHeight(options().length)}
            >
              <div class="flex min-h-0 grow flex-col">
                <PickerList
                  options={options()}
                  at={p().at}
                  current={currentIdx()}
                  choose={(idx) => {
                    const q = p();
                    if (q.kind === 'copy')
                      update({ ...s(), picker: null, film: copyGroup(s().film, q.tool, idx) });
                    else if (q.kind === 'open-recipe') {
                      const chosen = CARD_RECIPES[idx];
                      if (!chosen) return update({ ...s(), picker: null });
                      const entry = s().films.find((f) => f.id === chosen.filmId) ?? s().films[0];
                      update({
                        ...s(),
                        picker: null,
                        slot: chosen.slot,
                        recipeName: chosen.name,
                        recipeAuthor: chosen.author,
                        recipe: { ...chosen.recipe },
                        savedRecipe: { ...chosen.recipe },
                        savedFilmId: entry.id,
                        filmId: entry.id,
                        film: cloneFilm(entry.film),
                      });
                    } else if (q.kind === 'compare') {
                      update({
                        ...s(),
                        picker: null,
                        compare: {
                          b: idx === 0 ? 'opened' : idx - 1,
                          split: s().compare?.split ?? SPLIT,
                        },
                      });
                    } else if (q.kind === 'raw-option') {
                      const raw = [...s().raw];
                      raw[q.index] = idx;
                      update({ ...s(), picker: null, raw });
                    } else if (q.kind === 'setting-list') {
                      update({
                        ...s(),
                        picker: null,
                        filmOnly: false,
                        recipe: { ...s().recipe, [q.settingId]: idx },
                      });
                    }
                  }}
                />
              </div>
            </AnchoredFlyOut>
          );
        }}
      </Show>

      {/* Save Film Changes? confirmation on film switch */}
      <Show when={s().switchModal}>
        <FilmSwitchModal s={s()} update={update} />
      </Show>

      {/* Reflect Shooting Conditions? confirmation when edits would be lost */}
      <Show when={s().reflectModal}>
        <ReflectConfirmModal s={s()} update={update} />
      </Show>

      {/* Focal UI Text Keyboard */}
      <Show when={s().keyboard}>{(t) => <Keyboard s={s()} t={t()} update={update} />}</Show>
    </div>
  );
}
