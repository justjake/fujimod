import { For, Show, createEffect, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { palette, hueColor } from './palette';
import { WORDMARK_CAP_ROWS, WORDMARK_HEIGHT, WORDMARK_PATH, WORDMARK_WIDTH } from './wordmark';

/* ---- Icons ------------------------------------------------------------------ */

export function Disclosure(props: { dim?: boolean }) {
  return (
    <svg
      width="12"
      height="20"
      viewBox="0 0 12 20"
      class={props.dim ? 'text-dim' : 'text-disclosure'}
    >
      <path
        d="M2 2l8 8-8 8"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

function BackChevron() {
  return (
    <svg width="14" height="22" viewBox="0 0 14 22" class="text-label">
      <path
        d="M11 2l-9 9 9 9"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function Check(props: { size?: number; class?: string }) {
  const size = () => props.size ?? 20;
  return (
    <svg width={size()} height={size()} viewBox="0 0 20 20" class={props.class ?? 'text-setup'}>
      <path
        d="M3 10.5l4.5 4.5L17 5"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

/** A stock-style key glyph: white key, black legend. */
export function Key(props: { children: JSX.Element }) {
  return (
    <span class="inline-flex h-[26px] min-w-[26px] items-center justify-center rounded-[5px] bg-key px-[7px] align-middle text-key font-semibold text-key-legend">
      {props.children}
    </span>
  );
}

/** The rear command dial, pressed. */
export function DialKey() {
  return (
    <span class="inline-flex size-[26px] items-center justify-center rounded-[5px] bg-key align-middle">
      <svg width="18" height="18" viewBox="0 0 18 18">
        <circle cx="9" cy="9" r="7" fill="none" stroke="black" stroke-width="2.4" />
        <circle cx="9" cy="9" r="2.4" fill="black" />
      </svg>
    </span>
  );
}

export function SdCard() {
  return (
    <svg viewBox="0 0 17 21" class="inline-block h-[0.95em] w-[0.77em] align-[-0.16em]">
      <path d="M0 1a1 1 0 011-1h10l6 6v14a1 1 0 01-1 1H1a1 1 0 01-1-1z" fill="white" />
      <text
        x="7.5"
        y="15.5"
        text-anchor="middle"
        font-size="8.5"
        font-weight="800"
        font-family="Inter Variable, sans-serif"
        fill="black"
      >
        SD
      </text>
    </svg>
  );
}

/** Inter's cap height, in em. */
const CAP_HEIGHT = 0.727;

/** The FujiMod pixel wordmark set as a word in the surrounding text: its
 * capitals are the font's cap height and its baseline sits on the text's, the
 * j's descender below it. */
export function Wordmark() {
  const pixel = CAP_HEIGHT / WORDMARK_CAP_ROWS;
  return (
    <svg
      role="img"
      aria-label="FujiMod"
      viewBox={`0 0 ${WORDMARK_WIDTH} ${WORDMARK_HEIGHT}`}
      class="inline-block fill-current"
      style={{
        width: `${WORDMARK_WIDTH * pixel}em`,
        height: `${WORDMARK_HEIGHT * pixel}em`,
        'vertical-align': `-${(WORDMARK_HEIGHT - WORDMARK_CAP_ROWS) * pixel}em`,
      }}
    >
      <path d={WORDMARK_PATH} />
    </svg>
  );
}

/** The wordmark and a version, set as one line of text. */
export function Release(props: { version: string; class?: string }) {
  return (
    <div class={`leading-none font-bold whitespace-nowrap ${props.class ?? ''}`}>
      <Wordmark /> {props.version}
    </div>
  );
}

export function Spinner() {
  return (
    <svg width="44" height="44" viewBox="0 0 44 44">
      <For each={Array.from({ length: 8 }, (_, i) => i)}>
        {(i) => (
          <line
            x1="22"
            y1="6"
            x2="22"
            y2="14"
            stroke="white"
            stroke-width="4.5"
            stroke-linecap="round"
            opacity={0.2 + (0.8 * i) / 7}
            transform={`rotate(${i * 45} 22 22)`}
          />
        )}
      </For>
    </svg>
  );
}

export function Done() {
  return (
    <svg width="56" height="56" viewBox="0 0 56 56">
      <circle cx="28" cy="28" r="28" class="fill-success" />
      <path
        d="M16 29l8 8 16-17"
        fill="none"
        stroke="white"
        stroke-width="5.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

/* ---- Screen ----------------------------------------------------------------- */

export interface Overlay {
  /** Renders above the content, given the focused row's vertical center on screen. */
  render: (anchor: number) => JSX.Element;
}

/** A pushed Focal page on the 720x480 LCD: back chevron and title, then
 * content that scrolls the least distance needed to show the focused row. */
export function Screen(props: {
  title: string;
  hue?: number | null;
  accent?: string;
  /** Changes whenever focus moves, so the page re-scrolls. */
  focus?: string;
  /** Start at the top instead of scrolling to the focused row. */
  top?: boolean;
  overlay?: Overlay;
  children: JSX.Element;
}) {
  let content: HTMLDivElement | undefined;
  const [scrolled, setScrolled] = createSignal(false);
  const [thumb, setThumb] = createSignal<{ top: number; height: number } | null>(null);
  const [anchor, setAnchor] = createSignal(0);
  const colors = () => palette(props.hue ?? null);

  const measure = () => {
    if (!content) return;
    const visible = content.clientHeight;
    const total = content.scrollHeight;
    setScrolled(content.scrollTop > 0);
    if (total <= visible) return setThumb(null);
    const track = visible - 12;
    const height = Math.max(64, (track * visible) / total);
    setThumb({ top: 6 + ((track - height) * content.scrollTop) / (total - visible), height });
  };
  const follow = () => {
    if (!content) return;
    const focused = content.querySelector<HTMLElement>('[data-focused]');
    if (focused && !props.top) {
      const gap = 14;
      const bottom = focused.offsetTop + focused.offsetHeight + gap;
      if (bottom > content.scrollTop + content.clientHeight)
        content.scrollTop = bottom - content.clientHeight;
      if (focused.offsetTop - gap < content.scrollTop)
        content.scrollTop = Math.max(0, focused.offsetTop - gap);
    }
    if (focused) setAnchor(48 + focused.offsetTop - content.scrollTop + focused.offsetHeight / 2);
    measure();
  };
  onSettled(() => {
    requestAnimationFrame(follow);
  });
  createEffect(
    () => props.focus,
    () => {
      requestAnimationFrame(follow);
    },
    { defer: true },
  );

  return (
    <div
      class="focal-review relative h-screen-h w-screen-w overflow-hidden bg-(--page) text-label select-none"
      style={{
        '--page': colors().page,
        '--group': colors().group,
        '--hairline': colors().hairline,
        '--focus': colors().focus,
        '--accent': props.accent ?? 'var(--color-setup)',
      }}
    >
      <div
        ref={(element) => (content = element)}
        onScroll={measure}
        class="absolute inset-x-0 top-title bottom-0 overflow-y-auto [scrollbar-width:none]"
      >
        <div class="relative pb-[28px]">{props.children}</div>
      </div>
      <div class="absolute inset-x-0 top-0 flex h-title items-center gap-[14px] bg-(--page) pl-[22px]">
        <BackChevron />
        <span class="text-title uppercase">{props.title}</span>
      </div>
      <Show when={scrolled()}>
        <div class="pointer-events-none absolute inset-x-0 top-title h-[18px] bg-linear-to-b from-(--page) to-transparent" />
      </Show>
      <Show when={thumb()}>
        {(t) => (
          <div
            class="absolute right-[5px] w-[5px] rounded-full bg-white/50"
            style={{ top: `${48 + t().top}px`, height: `${t().height}px` }}
          />
        )}
      </Show>
      <Show when={props.overlay}>{(o) => <>{o().render(anchor())}</>}</Show>
    </div>
  );
}

/* ---- Groups and rows ---------------------------------------------------------- */

export function Group(props: { children: JSX.Element; class?: string }) {
  return (
    <div class={`mx-margin flex flex-col bg-(--group) ${props.class ?? ''}`}>{props.children}</div>
  );
}

export function Section(props: { title: string }) {
  return (
    <h2 class="mx-margin px-[6px] pt-[20px] pb-[12px] text-section font-bold">{props.title}</h2>
  );
}

/** One beveled row. `children` go below the label line (subtitles, sliders). */
export function Row(props: {
  label?: string;
  title?: string;
  value?: string;
  detail?: JSX.Element;
  trailing?: JSX.Element;
  focused?: boolean;
  disabled?: boolean;
  disclosure?: boolean;
  onSelect?: () => void;
  children?: JSX.Element;
}) {
  const disclosure = () => props.disclosure ?? true;
  const tall = () => Boolean(props.children || props.detail);
  return (
    <div
      data-focused={props.focused ? '' : undefined}
      onClick={() => props.onSelect?.()}
      class={`relative flex min-h-row flex-col justify-center pl-pad ${
        disclosure() ? 'pr-disclosure' : 'pr-pad'
      } ${props.focused ? 'focal-focused' : 'focal-bevel'} ${tall() ? 'pt-[14px] pb-[16px]' : ''}`}
    >
      <div class="flex items-center gap-[12px]">
        <Show when={props.label}>
          <span class={`text-row uppercase ${props.disabled ? 'text-disabled' : ''}`}>
            {props.label}
          </span>
        </Show>
        <Show when={props.title}>
          <span class={`text-card font-bold uppercase ${props.disabled ? 'text-disabled' : ''}`}>
            {props.title}
          </span>
        </Show>
        <span class="flex-1" />
        <Show when={props.value}>
          <span class={`text-row uppercase ${props.disabled ? 'text-disabled' : ''}`}>
            {props.value}
          </span>
        </Show>
        {props.trailing}
      </div>
      <Show when={props.detail}>
        <div class={`mt-[8px] text-detail ${props.disabled ? 'text-dim' : 'text-secondary'}`}>
          {props.detail}
        </div>
      </Show>
      {props.children}
      <Show when={disclosure()}>
        {/* On a tall row the chevron stays on the title line. */}
        <span class={`absolute right-[16px] -translate-y-1/2 ${tall() ? 'top-[29px]' : 'top-1/2'}`}>
          <Disclosure dim={props.disabled} />
        </span>
      </Show>
    </div>
  );
}

/** A color well inset in its row by the same distance from top, right and
 * bottom, beveled like the rows: a light hairline on top, a dark one below,
 * and a slight fall-off from top to bottom. */
export function Swatch(props: { color: string }) {
  return (
    <span
      class="absolute top-[9px] right-[9px] bottom-[9px] w-[160px] border border-black"
      style={{
        background: `linear-gradient(rgb(255 255 255 / 0.1), rgb(0 0 0 / 0.14)), ${props.color}`,
        'box-shadow':
          'inset 0 1px 0 rgb(255 255 255 / 0.35), inset 0 -2px 0 rgb(0 0 0 / 0.3), 0 1px 0 rgb(255 255 255 / 0.06)',
      }}
    />
  );
}

/** A rectangular button with the X100VI bevel, filled. */
/** `value` follows the label as written, not uppercased (a size in MiB). */
export function Button(props: {
  label: string;
  value?: string;
  focused?: boolean;
  color?: string;
}) {
  return (
    <div
      data-focused={props.focused ? '' : undefined}
      class="relative mx-margin flex h-button items-center justify-center text-row uppercase"
      style={{
        background: props.color ?? 'var(--accent)',
        'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.25), inset 0 -2px 0 black',
      }}
    >
      <Show when={props.focused}>
        <span class="absolute top-[1px] bottom-[2px] left-0 w-stripe bg-white" />
      </Show>
      {props.label}
      <Show when={props.value}>
        <span class="ml-[0.4em] normal-case">{props.value}</span>
      </Show>
    </div>
  );
}

export function ProgressBar(props: { fraction: number }) {
  return (
    <div class="h-bar overflow-hidden rounded-full bg-track">
      <div class="h-full rounded-full bg-progress" style={{ width: `${props.fraction * 100}%` }} />
    </div>
  );
}

export type FileState = number | 'present';

/** One segment per release file, its width its share of the release; files the
 * SD card already holds start full, in gray. */
export function SegmentedBar(props: { files: { size: number; state: FileState }[] }) {
  return (
    <div class="flex h-bar gap-[3px]">
      <For each={props.files}>
        {(file) => (
          <div
            class="h-full overflow-hidden rounded-full bg-track"
            style={{ flex: `${file.size} 1 0` }}
          >
            <div
              class={`h-full rounded-full ${file.state === 'present' ? 'bg-present' : 'bg-progress'}`}
              style={{ width: `${file.state === 'present' ? 100 : file.state * 100}%` }}
            />
          </div>
        )}
      </For>
    </div>
  );
}

/* ---- Popups ------------------------------------------------------------------- */

function PopupFrame(props: {
  anchor: number;
  width: number;
  height: number;
  children: JSX.Element;
}) {
  const top = () => Math.max(52, Math.min(props.anchor - props.height / 2, 480 - props.height - 4));
  const pointer = () => Math.max(top() + 14, Math.min(props.anchor, top() + props.height - 14));
  return (
    <>
      <div class="absolute inset-0 bg-black/35" />
      <div
        class="absolute right-0 border border-r-0 border-popup-border bg-popup shadow-[0_8px_28px_rgb(0_0_0/0.6)]"
        style={{ top: `${top()}px`, width: `${props.width}px`, height: `${props.height}px` }}
      >
        {props.children}
      </div>
      <span
        class="absolute -translate-y-1/2 rotate-180"
        style={{ top: `${pointer()}px`, right: `${props.width + 6}px` }}
      >
        <svg width="12" height="20" viewBox="0 0 12 20" class="text-label">
          <path
            d="M2 2l8 8-8 8"
            fill="none"
            stroke="currentColor"
            stroke-width="3"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </span>
    </>
  );
}

/** Focal's choice popup: square, at the right edge, pointing at its row. */
export function ChoicePopup(props: {
  anchor: number;
  options: string[];
  cursor: number;
  current: number;
  description?: string;
}) {
  const height = () => props.options.length * 52 + (props.description ? 44 : 0);
  return (
    <PopupFrame anchor={props.anchor} width={props.description ? 400 : 300} height={height()}>
      <For each={props.options}>
        {(option, index) => (
          <div
            class={`flex h-popup-row items-center border-popup-separator pl-[14px] ${
              index() ? 'border-t' : ''
            } ${index() === props.cursor ? 'bg-focus' : ''}`}
          >
            <span class="w-[34px]">
              <Show when={index() === props.current}>
                <Check />
              </Show>
            </span>
            <span class="text-row uppercase">{option}</span>
          </div>
        )}
      </For>
      <Show when={props.description}>
        <div class="flex h-[44px] items-center border-t border-popup-separator px-pad text-detail text-secondary uppercase">
          {props.description}
        </div>
      </Show>
    </PopupFrame>
  );
}

/** BACKGROUND COLOR offers this many hues, evenly spaced. */
export const HUE_STEPS = 24;
export const HUE_STEP = 360 / HUE_STEPS;

/** BACKGROUND COLOR: a row of hue segments with the default gray first, in a
 * bar above its row. LEFT/RIGHT move through them and the page retints as the
 * selection moves. */
export function HuePopup(props: { anchor: number; hue: number | null }) {
  const height = 76;
  const above = () => props.anchor - 29 - 8 - height;
  const top = () => (above() >= 52 ? above() : props.anchor + 29 + 8);
  const selected = 'outline-[3px] outline-offset-[2px] outline-white';
  return (
    <>
      <div class="absolute inset-0 bg-black/35" />
      <div
        class="absolute inset-x-margin flex items-center gap-[14px] border border-popup-border bg-popup px-pad shadow-[0_8px_28px_rgb(0_0_0/0.6)]"
        style={{ top: `${top()}px`, height: `${height}px` }}
      >
        <span
          class={`size-[44px] shrink-0 border border-black ${props.hue === null ? selected : ''}`}
          style={{ background: hueColor(null) }}
        />
        <div class="flex h-[28px] flex-1 gap-[2px]">
          <For each={Array.from({ length: HUE_STEPS }, (_, i) => i * HUE_STEP)}>
            {(hue) => (
              <span
                class={`flex-1 border border-black ${props.hue === hue ? selected : ''}`}
                style={{ background: hueColor(hue) }}
              />
            )}
          </For>
        </div>
      </div>
    </>
  );
}

export const SWATCHES = [
  '#b91000',
  '#e85c00',
  '#d6a000',
  '#008c32',
  '#009688',
  '#0068d8',
  '#5856d6',
  '#963cc8',
  '#d6286e',
  '#78787e',
  '#a07850',
  '#e6e6e6',
];

export function SwatchPopup(props: { anchor: number; cursor: number; current: number }) {
  return (
    <PopupFrame anchor={props.anchor} width={352} height={172}>
      <div class="grid grid-cols-4 gap-[10px] p-[13px]">
        <For each={SWATCHES}>
          {(color, index) => (
            <div
              class={`relative flex h-[40px] items-center justify-center border border-black ${
                index() === props.cursor ? 'outline-[3px] outline-offset-[3px] outline-white' : ''
              }`}
              style={{ background: color, 'box-shadow': 'inset 0 1px 0 rgb(255 255 255 / 0.2)' }}
            >
              <Show when={index() === props.current}>
                <Check
                  class={index() === SWATCHES.length - 1 ? 'text-black' : 'text-white'}
                  size={24}
                />
              </Show>
            </div>
          )}
        </For>
      </div>
    </PopupFrame>
  );
}
