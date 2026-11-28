import { For, Show, createEffect, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Artwork, Monogram, Wordmark, stockArtwork } from '../../../design/artwork';
import { Scroller } from '../../../design/column';
import { Chevron, Choice, Dial, signed, type ChoiceItem } from '../../../design/controls';
import { CHOSEN, COLUMN, GLASS, SELECTED, SMALL, TEXT, TRACK } from '../../../design/materials';
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
  rowsByIcon,
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

const AMBER = 'var(--m-mark)';
const ROW_H = 50;
const ICON_COL = 44;
const ROW_INSET = { top: 3, bottom: 3, right: 3 };

const GROUP_CARD_STYLE: JSX.CSSProperties = {
  background: 'var(--m-group)',
  'box-shadow': 'var(--m-group-edge)',
  'margin-inline': 'var(--m-group-margin, 0px)',
};

const CELL = (focused: boolean): JSX.CSSProperties => ({
  background: focused ? 'var(--m-focus)' : 'var(--m-cell)',
  'border-radius': '11px',
  'box-shadow': focused ? 'var(--m-focus-edge)' : 'var(--m-cell-edge)',
});

const RECESSED_WELL_STYLE: JSX.CSSProperties = {
  background: 'var(--m-subpanel)',
  'box-shadow': 'var(--m-subpanel-edge)',
};

const RAIL_CAPSULE_STYLE: JSX.CSSProperties = {
  background: 'var(--m-subpanel)',
  'box-shadow': 'var(--m-rail-capsule-edge)',
};

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
          class="-mx-[8px] h-[34px] w-[54px]"
          color={props.color}
          fallback={word(stock())}
        />
      )}
    </Show>
  );
}

/* ---- Collapsible top-level section header ---------------------------------------- */

function CollapsibleHeader(props: {
  title: string;
  collapsed: boolean;
  focused?: boolean;
  onToggle: () => void;
  accessory?: JSX.Element;
}) {
  return (
    <div class="mb-[3px] flex h-[22px] items-center gap-[4px] px-[2px]">
      <button
        type="button"
        class="flex min-w-0 grow items-center gap-[5px] rounded-[6px] px-[3px] py-[2px] text-left"
        style={props.focused ? TRACK(true) : {}}
        onClick={() => props.onToggle()}
      >
        <Chevron
          dir={props.collapsed ? 'right' : 'down'}
          size={11}
          color="rgba(255,255,255,0.58)"
        />
        <span class="truncate text-[12px] leading-none font-bold tracking-[0.08em] text-white/65">
          {props.title}
        </span>
      </button>
      {props.accessory}
    </div>
  );
}

/* ---- Compact icon-left setting row (Light / Color / Detail / Effects) ------------ */

function CompactSettingRow(props: {
  setting: RecipeSetting;
  s: UnifiedState;
  update: (s: UnifiedState) => void;
}) {
  const controlWidth = CONTENT - ICON_COL - ROW_INSET.right;
  const controlHeight = ROW_H - ROW_INSET.top - ROW_INSET.bottom;
  const focused = () => props.s.focus.kind === 'setting' && props.s.focus.id === props.setting.id;
  const changed = () => props.s.recipe[props.setting.id] !== props.s.savedRecipe[props.setting.id];
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

  const items: ChoiceItem[] =
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
                    class="h-[22px] w-[36px]"
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
      class="relative flex items-stretch"
      style={{
        height: `${ROW_H}px`,
        'padding-top': `${ROW_INSET.top}px`,
        'padding-bottom': `${ROW_INSET.bottom}px`,
        'padding-right': `${ROW_INSET.right}px`,
      }}
    >
      <Show
        when={props.setting.kind === 'slider' ? (props.setting as SliderSetting) : undefined}
        fallback={
          <Choice
            items={items}
            value={props.s.recipe[props.setting.id] ?? 0}
            width={controlWidth}
            height={controlHeight}
            focused={focused()}
            changed={changed()}
            pick={pick}
            onFocus={focus}
          />
        }
      >
        {(slider) => (
          <Dial
            value={props.s.recipe[slider().id] ?? 0}
            min={0}
            max={slider().steps.length - 1}
            step={1}
            zero={Math.max(0, slider().steps.indexOf(0))}
            format={(i) => fmtStep(slider().steps[i])}
            width={controlWidth}
            height={controlHeight}
            focused={focused()}
            changed={changed()}
            set={pick}
            onFocus={focus}
          />
        )}
      </Show>
    </div>
  );
}

function SettingIconGroup(props: {
  rows: RecipeSetting[];
  s: UnifiedState;
  update: (s: UnifiedState) => void;
}) {
  const hasFocus = () =>
    props.s.focus.kind === 'setting' &&
    props.rows.some((r) => r.id === (props.s.focus as { kind: 'setting'; id: string }).id);
  const hasWb = () => props.rows.some((r) => r.id === 'wb');
  const wbShifted = () => props.s.recipe.shiftR !== 0 || props.s.recipe.shiftB !== 0;
  return (
    <div class="relative flex items-start">
      <div
        class="flex shrink-0 flex-col items-center justify-center pt-[4px]"
        style={{ width: `${ICON_COL}px`, 'min-height': `${ROW_H}px` }}
        onClick={() => {
          if (hasWb()) {
            props.update({
              ...props.s,
              filmOnly: false,
              focus: { kind: 'setting', id: 'wb' },
              wbShift: {
                open: !props.s.wbShift.open,
                from: [props.s.recipe.shiftR, props.s.recipe.shiftB],
              },
            });
          } else {
            props.update({
              ...props.s,
              filmOnly: false,
              focus: { kind: 'setting', id: props.rows[0].id },
            });
          }
        }}
      >
        <RowIconView
          icon={props.rows[0].icon}
          color={hasFocus() ? '#fff' : 'rgb(255 255 255 / 0.7)'}
        />
        <Show when={hasWb() && wbShifted()}>
          <span class="mt-[1px] text-[9px] leading-none font-semibold tracking-[-0.02em] text-white/75 tabular-nums">
            {signed(props.s.recipe.shiftR)},{signed(props.s.recipe.shiftB)}
          </span>
        </Show>
      </div>
      <div class="flex min-w-0 flex-1 flex-col">
        <For each={props.rows}>
          {(setting) => <CompactSettingRow setting={setting} s={props.s} update={props.update} />}
        </For>
      </div>
    </div>
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

  const toolFrame = (focused: boolean): JSX.CSSProperties => (focused ? TRACK(true) : {});

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
        {/* 60px Quick-Scroll Rail */}
        <div
          class="absolute inset-y-0 left-0 flex flex-col items-center gap-[3px] overflow-y-auto pt-[6px] pb-[8px]"
          style={{ ...COLUMN, width: `${RAIL}px`, 'scrollbar-width': 'none' }}
        >
          <For each={topSections()}>
            {(sec) => {
              const isChosen = () => railActive() === sec;
              const isLit = () => isRailFocused(sec);
              const dimmed = () => s().filmOnly && SETTING_GROUPS.includes(sec as SettingGroupId);
              const filmExpanded = () => sec === 'film' && !s().collapsed.film;
              const topBtn = (compact = false) => (
                <button
                  type="button"
                  aria-label={sec}
                  class={`relative flex shrink-0 items-center justify-center transition-opacity duration-150 ${compact ? 'size-[38px] rounded-[9px]' : 'size-[42px] rounded-[11px]'}`}
                  style={{
                    ...(isLit()
                      ? { ...SELECTED, 'box-shadow': 'var(--m-focus-edge)' }
                      : isChosen()
                        ? SELECTED
                        : {}),
                    opacity: dimmed() ? 0.42 : 1,
                  }}
                  onClick={() => {
                    scrollToSection(sec, true);
                    update(enterSection(s(), sec));
                  }}
                >
                  <RailIcon id={sec} color="#fff" size={compact ? 21 : 23} />
                  <Show when={sec === 'film' && dirtyFilm()}>
                    <span
                      class="absolute top-[3px] right-[3px] size-[6px] rounded-full"
                      style={{ background: AMBER, 'box-shadow': '0 0 0 1.5px #000' }}
                    />
                  </Show>
                </button>
              );
              return (
                <Show when={filmExpanded()} fallback={topBtn(false)}>
                  <div
                    class="my-[2px] flex flex-col items-center gap-[2px] rounded-[11px] p-[3px]"
                    style={RAIL_CAPSULE_STYLE}
                  >
                    {topBtn(true)}
                    <For each={FILM_TOOLS}>
                      {(tool) => {
                        const toolChosen = () => railActive() === tool;
                        const toolLit = () => isRailFocused(tool);
                        return (
                          <button
                            type="button"
                            aria-label={FILM_TOOL_NAMES[tool]}
                            class="relative flex size-[36px] shrink-0 items-center justify-center rounded-[9px]"
                            style={
                              toolLit()
                                ? { ...SELECTED, 'box-shadow': 'var(--m-focus-edge)' }
                                : toolChosen()
                                  ? SELECTED
                                  : {}
                            }
                            onClick={() => {
                              scrollToSection(tool, true);
                              update(enterSection(s(), tool));
                            }}
                          >
                            <RailIcon id={tool} color="rgba(255,255,255,0.9)" size={20} />
                            <Show when={isFilmToolEdited(s(), tool)}>
                              <span
                                class="absolute top-[2px] right-[2px] size-[6px] rounded-full"
                                style={{ background: AMBER, 'box-shadow': '0 0 0 1.5px #000' }}
                              />
                            </Show>
                          </button>
                        );
                      }}
                    </For>
                  </div>
                </Show>
              );
            }}
          </For>
        </div>

        {/* 2x2 or 3x3 Numbers sheet over scroller */}
        <Show when={s().numbers}>
          <NumbersSheet s={s()} update={update} />
        </Show>

        {/* 308px Scroller (50% total column with 52px rail) */}
        <Scroller
          ref={(element) => (scroller = element)}
          class="absolute inset-y-0 px-[6px]"
          style={{
            left: `${RAIL}px`,
            width: `${SCROLLER}px`,
            visibility: s().numbers ? 'hidden' : undefined,
          }}
        >
          {/* 1. [if mode === 'raw'] RAW CONVERSION (flush Focal Group in Fujimod, spaced single-stroke tiles in High Contrast / Glass) */}
          <Show when={s().mode === 'raw'}>
            <section ref={(el) => (sectionRefs.raw = el)} class="pt-[5px] pb-[6px]">
              <CollapsibleHeader
                title="RAW CONVERSION"
                collapsed={s().collapsed.raw}
                onToggle={() => toggleSection('raw')}
              />
              <Show when={!s().collapsed.raw}>
                <div
                  class="flex flex-col"
                  style={{ ...GROUP_CARD_STYLE, gap: 'var(--m-group-gap)' }}
                >
                  <div
                    class="flex h-[46px] items-center gap-[8px] px-[10px]"
                    style={CELL(isRawFocused('strip'))}
                    onPointerDown={() => update({ ...s(), focus: { kind: 'raw', item: 'strip' } })}
                  >
                    <div class="flex min-w-0 grow flex-col gap-[2px]">
                      <span class="truncate text-[18px] leading-none font-semibold text-white">
                        {RAWS[s().crop - 1].name}
                      </span>
                      <span class="truncate text-[13px] leading-none text-white/60 tabular-nums">
                        {RAWS[s().crop - 1].info}
                      </span>
                    </div>
                    <span class="shrink-0 text-[16px] leading-none text-white/70 tabular-nums">
                      {s().crop}/{RAWS.length}
                    </span>
                  </div>
                  {/* 2x3 Q-menu style tile grid: < / > cycles inline, tap / OK opens picker flyout */}
                  <div class="grid grid-cols-2" style={{ gap: 'var(--m-group-gap)' }}>
                    <For each={RAW_OPTIONS}>
                      {(o, k) => {
                        const active = () => isRawFocused(k());
                        const valIdx = () => s().raw[k()] ?? 0;
                        const changed = () => valIdx() !== RAW_DEFAULT[k()];
                        return (
                          <button
                            type="button"
                            data-anchor={`raw-${k()}`}
                            class="relative flex h-[44px] flex-col items-start justify-center gap-[3px] px-[8px] text-left"
                            style={CELL(active())}
                            onClick={() =>
                              update({
                                ...s(),
                                focus: { kind: 'raw', item: k() },
                                picker: { kind: 'raw-option', index: k(), at: valIdx() },
                              })
                            }
                          >
                            <div class="flex w-full items-center justify-between gap-[4px]">
                              <span class="truncate text-[11px] leading-none font-semibold tracking-[0.05em] text-white/55">
                                {o.short}
                              </span>
                              <Show when={changed()}>
                                <span
                                  class="size-[5px] shrink-0 rounded-full"
                                  style={{ background: AMBER }}
                                />
                              </Show>
                            </div>
                            <div class="flex w-full items-center justify-between gap-[4px]">
                              <span class="truncate text-[16px] leading-none font-semibold text-white">
                                {o.values[valIdx()]}
                              </span>
                              <Chevron dir="right" size={11} color="rgba(255,255,255,0.45)" />
                            </div>
                          </button>
                        );
                      }}
                    </For>
                  </div>
                  <div class="flex" style={{ gap: 'var(--m-group-gap)' }}>
                    <button
                      type="button"
                      class="flex h-[36px] min-w-0 grow items-center justify-center px-[6px] text-[13px] leading-none font-semibold whitespace-nowrap text-white"
                      style={CELL(isRawFocused('reflect'))}
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
                      class="flex h-[36px] min-w-0 grow items-center justify-center px-[8px] text-[14px] leading-none font-semibold whitespace-nowrap text-white"
                      style={{
                        ...CELL(isRawFocused('create')),
                        ...(s().created === s().crop ? { background: 'var(--m-accent)' } : {}),
                      }}
                      onClick={() =>
                        update({
                          ...s(),
                          created: s().crop,
                          focus: { kind: 'raw', item: 'create' },
                        })
                      }
                    >
                      {s().created === s().crop ? 'CREATED' : 'CREATE JPEG'}
                    </button>
                  </div>
                </div>
              </Show>
            </section>
          </Show>

          {/* 2. RECIPE */}
          <section ref={(el) => (sectionRefs.recipe = el)} class="pt-[5px] pb-[6px]">
            <CollapsibleHeader
              title="RECIPE"
              collapsed={s().collapsed.recipe}
              onToggle={() => toggleSection('recipe')}
            />
            <Show when={!s().collapsed.recipe}>
              <div class="flex flex-col" style={{ ...GROUP_CARD_STYLE, gap: 'var(--m-group-gap)' }}>
                <div
                  class="flex h-[48px] items-center gap-[8px] px-[8px]"
                  style={CELL(isRecipeFocused('name'))}
                  onClick={() =>
                    update({
                      ...s(),
                      focus: { kind: 'recipe', item: 'name' },
                      keyboard: typing(s().recipeName, 'recipe-name'),
                    })
                  }
                >
                  <span
                    class="flex h-[30px] min-w-[34px] shrink-0 items-center justify-center rounded-[7px] px-[5px] text-[16px] leading-none font-bold"
                    style={CHOSEN}
                  >
                    {s().slot}
                  </span>
                  <div class="flex min-w-0 grow flex-col gap-[2px]">
                    <span class="truncate text-[18px] leading-none font-semibold text-white">
                      {s().recipeName}
                    </span>
                    <span class="truncate text-[13px] leading-none text-white/58">
                      {s().recipeAuthor} · {dirtyRecipe() ? 'edited' : 'saved'}
                    </span>
                  </div>
                  <svg width="17" height="17" viewBox="0 0 24 24" class="shrink-0 opacity-65">
                    <path
                      d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4"
                      fill="none"
                      stroke="#fff"
                      stroke-width="2"
                      stroke-linejoin="round"
                    />
                  </svg>
                </div>
                <div class="flex" style={{ gap: 'var(--m-group-gap)' }}>
                  <button
                    type="button"
                    class="flex h-[36px] grow basis-0 items-center justify-center px-[6px] text-[14px] leading-none font-semibold whitespace-nowrap"
                    style={{
                      ...CELL(isRecipeFocused('save')),
                      color: !dirtyRecipe() ? 'rgb(255 255 255 / 0.35)' : '#fff',
                    }}
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
                    class="flex h-[36px] grow items-center justify-center px-[6px] text-[14px] leading-none font-semibold whitespace-nowrap text-white"
                    style={CELL(isRecipeFocused('saveas'))}
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
                    class="flex h-[36px] grow basis-0 items-center justify-center px-[6px] text-[14px] leading-none font-semibold whitespace-nowrap text-white"
                    style={CELL(isRecipeFocused('open'))}
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
              </div>
            </Show>
          </section>

          {/* 3. FILM SIMULATION — picker sits clean without double-carding; expanded tools sit in darker recessed well below */}
          <section ref={(el) => (sectionRefs.film = el)} class="pt-[5px] pb-[6px]">
            <CollapsibleHeader
              title="FILM SIMULATION"
              collapsed={s().collapsed.film}
              focused={s().focus.kind === 'film-header'}
              onToggle={() => toggleSection('film')}
            />

            {/* Horizontal film tile picker — exact same icon gutter, Choice size, and 40x24 stock film icons as other setting rows */}
            <div class="relative flex items-start">
              <div
                class="flex shrink-0 items-center justify-center"
                style={{ width: `${ICON_COL}px`, height: `${ROW_H}px` }}
                onClick={() => update({ ...s(), focus: { kind: 'film-tiles' } })}
              >
                <RowIconView
                  icon={{ stock: 0x5f3 }}
                  color={s().focus.kind === 'film-tiles' ? '#fff' : 'rgb(255 255 255 / 0.72)'}
                />
              </div>
              <div
                class="relative flex min-w-0 flex-1 items-stretch"
                style={{
                  height: `${ROW_H}px`,
                  'padding-top': `${ROW_INSET.top}px`,
                  'padding-bottom': `${ROW_INSET.bottom}px`,
                  'padding-right': `${ROW_INSET.right}px`,
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
                  width={CONTENT - ICON_COL - ROW_INSET.right}
                  height={ROW_H - ROW_INSET.top - ROW_INSET.bottom}
                  focused={s().focus.kind === 'film-tiles'}
                  changed={s().filmId !== s().savedFilmId}
                  pick={(idx) => {
                    const target = s().films[idx];
                    if (target)
                      update(
                        requestFilmSelect({ ...s(), focus: { kind: 'film-tiles' } }, target.id),
                      );
                  }}
                  onFocus={() => update({ ...s(), focus: { kind: 'film-tiles' } })}
                />
              </div>
            </div>

            {/* Darker recessed Film Simulation sub-editor well directly below the film picker (zero double-carding) */}
            <Show when={!s().collapsed.film}>
              <div
                class="-mx-[6px] mt-[6px] px-[6px] pt-[8px] pb-[8px]"
                style={RECESSED_WELL_STYLE}
              >
                {/* Minimal plain-English film file action row ONLY when modified */}
                <Show when={dirtyFilm()}>
                  <div
                    class="mb-[8px] flex"
                    style={{ ...GROUP_CARD_STYLE, gap: 'var(--m-group-gap)' }}
                  >
                    <Show when={activeFilm().isCard}>
                      <button
                        type="button"
                        class="flex h-[34px] min-w-0 grow items-center justify-center px-[6px] text-[12px] leading-none font-semibold whitespace-nowrap text-white"
                        style={CELL(isFilmActionFocused('update'))}
                        onClick={() => update(updateCardFilm(s()))}
                      >
                        <span class="truncate">UPDATE {activeFilm().short}</span>
                      </button>
                    </Show>
                    <button
                      type="button"
                      class="flex h-[34px] shrink-0 items-center justify-center px-[8px] text-[12px] leading-none font-semibold whitespace-nowrap text-white"
                      style={CELL(isFilmActionFocused('saveas'))}
                      onClick={() => update(openSaveFilmKeyboard(s()))}
                    >
                      SAVE AS FILM
                    </button>
                    <button
                      type="button"
                      class="flex h-[34px] shrink-0 items-center justify-center px-[8px] text-[12px] leading-none font-medium whitespace-nowrap text-white/75"
                      style={CELL(isFilmActionFocused('reset'))}
                      onClick={() => update(resetFilmEdits(s()))}
                    >
                      RESET
                    </button>
                  </div>
                </Show>

                {/* 3a. TONE CURVE */}
                <div ref={(el) => (sectionRefs.curve = el)} class="pb-[10px]">
                  <div class="mb-[4px] flex h-[28px] items-center gap-[6px] px-[2px]">
                    <span class="grow text-[14px] leading-none font-semibold tracking-[0.05em] text-white/80">
                      TONE CURVE
                    </span>
                    <button
                      type="button"
                      data-anchor="copy-curve"
                      class="flex h-[26px] shrink-0 items-center gap-[4px] rounded-[7px] pr-[6px] pl-[8px] text-[13px] leading-none font-medium whitespace-nowrap"
                      style={{
                        background: 'var(--m-chip)',
                        color: 'rgb(255 255 255 / 0.85)',
                        'box-shadow': isToolHeaderFocused('curve')
                          ? 'var(--m-focus-edge)'
                          : 'var(--m-chip-edge)',
                      }}
                      onClick={() => {
                        const src = groupSource(s().film, 'curve') ?? 0;
                        update({
                          ...s(),
                          focus: { kind: 'film-tool-header', tool: 'curve' },
                          picker: { kind: 'copy', tool: 'curve', at: src },
                        });
                      }}
                    >
                      <Show when={groupSource(s().film, 'curve') === undefined}>
                        <span class="size-[6px] rounded-full" style={{ background: AMBER }} />
                      </Show>
                      {groupSource(s().film, 'curve') === undefined
                        ? 'EDITED'
                        : BASES[groupSource(s().film, 'curve') as number].name}
                      <Chevron dir="down" size={11} />
                    </button>
                  </div>

                  <div
                    class="relative rounded-[11px] p-[4px]"
                    style={{ background: 'rgba(0,0,0,0.28)', ...toolFrame(isCurveFocused()) }}
                    onClick={() => update({ ...s(), focus: { kind: 'film-curve' } })}
                  >
                    {/* Expand button + ALL | R | G | B channel tabs right inside the curve card */}
                    <div class="flex items-center justify-between px-[4px] pt-[2px] pb-[2px]">
                      <button
                        type="button"
                        class="flex size-[26px] items-center justify-center rounded-[6px]"
                        style={{ background: 'rgb(255 255 255 / 0.12)' }}
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
                      width={CONTENT - 8}
                      height={148}
                      focus={s().inside && isCurveFocused() ? s().knot : -1}
                      bend={s().inside && isCurveFocused() && s().bend}
                      set={(film) => update({ ...s(), film })}
                      focusKnot={(knot) =>
                        update({ ...s(), knot, focus: { kind: 'film-curve' }, inside: true })
                      }
                      active={s().inside && isCurveFocused()}
                    />
                    <div class="h-[20px] px-[6px] pt-[2px] text-[13px] leading-none font-medium text-white/80 tabular-nums">
                      {s().inside && isCurveFocused()
                        ? knotReadout(s().film, s().curveChannel, s().knot, s().bend)
                        : ''}
                    </div>
                  </div>
                </div>

                {/* 3b. CHROMA */}
                <div ref={(el) => (sectionRefs.chroma = el)} class="pt-[8px] pb-[8px]">
                  <div class="mb-[4px] flex h-[28px] items-center gap-[6px] px-[2px]">
                    <span class="grow text-[14px] leading-none font-semibold tracking-[0.05em] text-white/80">
                      CHROMA
                    </span>
                    <button
                      type="button"
                      data-anchor="copy-chroma"
                      class="flex h-[26px] shrink-0 items-center gap-[4px] rounded-[7px] pr-[6px] pl-[8px] text-[13px] leading-none font-medium whitespace-nowrap"
                      style={{
                        background: 'var(--m-chip)',
                        color: 'rgb(255 255 255 / 0.85)',
                        'box-shadow': isToolHeaderFocused('chroma')
                          ? 'var(--m-focus-edge)'
                          : 'var(--m-chip-edge)',
                      }}
                      onClick={() => {
                        const src = groupSource(s().film, 'chroma') ?? 0;
                        update({
                          ...s(),
                          focus: { kind: 'film-tool-header', tool: 'chroma' },
                          picker: { kind: 'copy', tool: 'chroma', at: src },
                        });
                      }}
                    >
                      <Show when={groupSource(s().film, 'chroma') === undefined}>
                        <span class="size-[6px] rounded-full" style={{ background: AMBER }} />
                      </Show>
                      {groupSource(s().film, 'chroma') === undefined
                        ? 'EDITED'
                        : BASES[groupSource(s().film, 'chroma') as number].name}
                      <Chevron dir="down" size={11} />
                    </button>
                  </div>

                  <ColorWheel film={s().film} set={(film) => update({ ...s(), film })} />
                  <For each={CHROMA_SCALES}>
                    {(c, k) => (
                      <div class="py-[2px]">
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
                      </div>
                    )}
                  </For>
                  <button
                    type="button"
                    class={`my-[2px] flex h-[44px] w-full items-center justify-between rounded-[10px] px-[12px] text-white/55 ${TEXT}`}
                    style={isChromaFocused(3) ? { ...TRACK(true), color: '#fff' } : {}}
                    onClick={() =>
                      update({
                        ...s(),
                        focus: { kind: 'film-chroma', row: 3 },
                        numbers: { tool: 'chroma', cell: 0 },
                      })
                    }
                  >
                    NUMBERS (2×2)
                    <Chevron dir="right" size={14} />
                  </button>
                </div>

                {/* 3c. COLOR MIX */}
                <div ref={(el) => (sectionRefs.mix = el)} class="pt-[8px] pb-[8px]">
                  <div class="mb-[4px] flex h-[28px] items-center gap-[6px] px-[2px]">
                    <span class="grow text-[14px] leading-none font-semibold tracking-[0.05em] text-white/80">
                      COLOR MIX
                    </span>
                    <button
                      type="button"
                      data-anchor="copy-mix"
                      class="flex h-[26px] shrink-0 items-center gap-[4px] rounded-[7px] pr-[6px] pl-[8px] text-[13px] leading-none font-medium whitespace-nowrap"
                      style={{
                        background: 'var(--m-chip)',
                        color: 'rgb(255 255 255 / 0.85)',
                        'box-shadow': isToolHeaderFocused('mix')
                          ? 'var(--m-focus-edge)'
                          : 'var(--m-chip-edge)',
                      }}
                      onClick={() => {
                        const src = groupSource(s().film, 'mix') ?? 0;
                        update({
                          ...s(),
                          focus: { kind: 'film-tool-header', tool: 'mix' },
                          picker: { kind: 'copy', tool: 'mix', at: src },
                        });
                      }}
                    >
                      <Show when={groupSource(s().film, 'mix') === undefined}>
                        <span class="size-[6px] rounded-full" style={{ background: AMBER }} />
                      </Show>
                      {groupSource(s().film, 'mix') === undefined
                        ? 'EDITED'
                        : BASES[groupSource(s().film, 'mix') as number].name}
                      <Chevron dir="down" size={11} />
                    </button>
                  </div>

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
                    class="my-[2px] flex gap-[4px] rounded-[10px] p-[4px]"
                    style={TRACK(isMixFocused('chips'))}
                    onPointerDown={() =>
                      update({ ...s(), focus: { kind: 'film-mix', row: 'chips' } })
                    }
                  >
                    <For each={MIX}>
                      {(mix, p) => (
                        <button
                          type="button"
                          class="flex h-[34px] flex-1 items-center justify-center gap-[5px] rounded-[8px] text-[16px] leading-none font-medium"
                          style={
                            s().primary === p() ? CHOSEN : { color: 'rgb(255 255 255 / 0.75)' }
                          }
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
                        <div class="py-[2px]">
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
                        </div>
                      );
                    }}
                  </For>
                  <button
                    type="button"
                    class="my-[2px] flex h-[36px] w-full items-center justify-between rounded-[9px] px-[10px] text-[15px] leading-none text-white/60"
                    style={isMixFocused('numbers') ? { ...TRACK(true), color: '#fff' } : {}}
                    onClick={() =>
                      update({
                        ...s(),
                        focus: { kind: 'film-mix', row: 'numbers' },
                        numbers: { tool: 'mix', cell: 1 },
                      })
                    }
                  >
                    NUMBERS (3×3)
                    <Chevron dir="right" size={13} />
                  </button>
                </div>

                {/* 3d. LUMA SAT */}
                <div ref={(el) => (sectionRefs.luma = el)} class="pt-[8px] pb-[4px]">
                  <div class="mb-[4px] flex h-[28px] items-center gap-[6px] px-[2px]">
                    <span class="grow text-[14px] leading-none font-semibold tracking-[0.05em] text-white/80">
                      LUMA SAT
                    </span>
                    <button
                      type="button"
                      data-anchor="copy-luma"
                      class="flex h-[26px] shrink-0 items-center gap-[4px] rounded-[7px] pr-[6px] pl-[8px] text-[13px] leading-none font-medium whitespace-nowrap"
                      style={{
                        background: 'var(--m-chip)',
                        color: 'rgb(255 255 255 / 0.85)',
                        'box-shadow': isToolHeaderFocused('luma')
                          ? 'var(--m-focus-edge)'
                          : 'var(--m-chip-edge)',
                      }}
                      onClick={() => {
                        const src = groupSource(s().film, 'luma') ?? 0;
                        update({
                          ...s(),
                          focus: { kind: 'film-tool-header', tool: 'luma' },
                          picker: { kind: 'copy', tool: 'luma', at: src },
                        });
                      }}
                    >
                      <Show when={groupSource(s().film, 'luma') === undefined}>
                        <span class="size-[6px] rounded-full" style={{ background: AMBER }} />
                      </Show>
                      {groupSource(s().film, 'luma') === undefined
                        ? 'EDITED'
                        : BASES[groupSource(s().film, 'luma') as number].name}
                      <Chevron dir="down" size={11} />
                    </button>
                  </div>

                  <div
                    class="rounded-[11px] p-[6px]"
                    style={{ background: 'rgba(0,0,0,0.28)', ...toolFrame(isLumaFocused()) }}
                    onClick={() => update({ ...s(), focus: { kind: 'film-luma' } })}
                  >
                    <ToneGraph
                      film={s().film}
                      opened={activeFilm().film}
                      width={CONTENT - 12}
                      height={132}
                      band={s().band}
                      inside={s().inside && isLumaFocused()}
                      set={(film) => update({ ...s(), film })}
                      focusBand={(band) =>
                        update({ ...s(), band, focus: { kind: 'film-luma' }, inside: true })
                      }
                    />
                    <div class="h-[20px] px-[4px] pt-[4px] text-[13px] leading-none font-medium text-white/80 tabular-nums">
                      {s().inside && isLumaFocused()
                        ? `${BAND_NAMES[s().band]} · ${signed(s().film.bands[s().band])}%`
                        : ''}
                    </div>
                  </div>
                </div>
              </div>
            </Show>
          </section>

          {/* 4-7. LIGHT, COLOR, DETAIL, EFFECTS (single-surface Focal Group in Fujimod, clean spaced rows in High Contrast / Glass) */}
          <For each={SETTING_GROUPS}>
            {(group) => (
              <section
                ref={(el) => (sectionRefs[group] = el)}
                class="pt-[5px] pb-[6px] transition-opacity duration-150"
                style={{ opacity: s().filmOnly ? 0.42 : 1 }}
              >
                <CollapsibleHeader
                  title={SETTING_GROUP_TITLES[group]}
                  collapsed={s().collapsed[group]}
                  onToggle={() => toggleSection(group)}
                />
                <Show when={!s().collapsed[group]}>
                  <div style={GROUP_CARD_STYLE}>
                    <For each={rowsByIcon(group)}>
                      {(rows) => <SettingIconGroup rows={rows} s={s()} update={update} />}
                    </For>
                  </div>
                </Show>
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
            {(t) => <Card class="px-[10px] py-[6px] text-[18px] leading-none">{t()}</Card>}
          </Show>
          <Show when={hudValue()}>
            {(v) => (
              <Card class="flex items-center gap-[6px] px-[10px] py-[6px] text-[17px] leading-none">
                <Show when={isInFilmArea(s().focus) && dirtyFilm()}>
                  <span class="size-[7px] shrink-0 rounded-full" style={{ background: AMBER }} />
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
              accent={s().filmOnly}
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
                accent={Boolean(s().compare)}
                tap={() => update(press(s(), 'q'))}
              />
              <KeyHint
                glyph="▶"
                label={s().mode === 'raw' ? 'LIVE' : 'RAW'}
                accent={s().mode === 'raw'}
                tap={() => update(press(s(), 'play'))}
              />
            </div>
          </Show>
        </div>
      </Show>

      {/* WB SHIFT 2D flyout */}
      <Show when={s().wbShift.open}>
        <FlyOut left={PICTURE_LEFT} rowTop={84} height={SHIFT_HEIGHT + SHIFT_CAPTION + 22}>
          <div class="flex flex-col items-center px-[10px] pt-[10px] pb-[2px]">
            <ShiftGrid
              r={s().recipe.shiftR}
              b={s().recipe.shiftB}
              onShift={(shiftR, shiftB) =>
                update({ ...s(), recipe: { ...s().recipe, shiftR, shiftB } })
              }
            />
            <span
              class={`flex h-[30px] items-center font-medium tracking-[0.04em] tabular-nums ${TEXT}`}
            >
              R {signed(s().recipe.shiftR)}&ensp;B {signed(s().recipe.shiftB)}
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
              class="flex size-[38px] items-center justify-center rounded-[10px]"
              style={GLASS}
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
            class={`absolute bottom-[16px] left-[24px] flex h-[36px] items-center rounded-[12px] px-[12px] font-medium tabular-nums ${SMALL}`}
            style={GLASS}
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
