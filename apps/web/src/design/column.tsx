import { Show } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { Chevron } from './controls';
import {
  BODY,
  CAPTION,
  COLUMN,
  DIM,
  EASE,
  EXPAND_MS,
  HAIRLINE,
  HEADER,
  MARGIN,
  ROW,
  TRACK,
  VALUE,
} from './materials';

/* The column every editor screen draws its rows in, and the pieces inside it. */

/** A column of rows over the picture: native scrolling (a finger, momentum)
 * without a scroll bar, in the column material. */
export function Scroller(props: {
  ref?: (element: HTMLDivElement) => void;
  class?: string;
  style?: JSX.CSSProperties;
  onScroll?: (top: number) => void;
  children: JSX.Element;
}) {
  return (
    <div
      ref={(element) => props.ref?.(element)}
      class={`overflow-y-auto overscroll-contain ${props.class ?? ''}`}
      style={{ ...COLUMN, 'scrollbar-width': 'none', ...props.style }}
      onScroll={(e) => props.onScroll?.(e.currentTarget.scrollTop)}
    >
      {props.children}
    </div>
  );
}

/** Scrolls `scroller` the least that shows `top` to `bottom` with `margin`
 * to spare: smoothly once the screen is showing, at once when it opens. */
/** `view` is the scroll position and height when the caller knows them, so
 * moving the focus reads no layout. */
export function reveal(
  scroller: HTMLElement,
  top: number,
  bottom: number,
  margin: number,
  smooth: boolean,
  view?: { top: number; height: number },
) {
  const behavior = smooth ? 'smooth' : 'auto';
  const at = view?.top ?? scroller.scrollTop;
  const height = view?.height ?? scroller.clientHeight;
  if (top < at + margin / 2) scroller.scrollTo({ top: top - margin, behavior });
  else if (bottom > at + height - margin / 2)
    scroller.scrollTo({ top: bottom - height + margin, behavior });
}

/** A section's header: its label on the left, what the closed section holds
 * on the right, and a chevron that turns as it opens. The whole row toggles. */
export function SectionHeader(props: {
  title: string;
  /** What the section holds, shown while it is closed. */
  summary?: JSX.Element;
  open: boolean;
  focused?: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      class="flex w-full items-center gap-[8px] text-left"
      style={{
        height: `${HEADER}px`,
        'padding-inline': `${MARGIN}px`,
        ...TRACK(Boolean(props.focused)),
      }}
      onClick={() => props.onToggle()}
    >
      <span class={`grow truncate font-semibold ${CAPTION}`} style={{ color: DIM }}>
        {props.title}
      </span>
      <Show when={!props.open && props.summary}>
        <span class={`min-w-0 truncate ${CAPTION}`} style={{ color: DIM }}>
          {props.summary}
        </span>
      </Show>
      <span
        class="flex shrink-0"
        style={{
          color: DIM,
          transform: props.open ? 'rotate(90deg)' : 'none',
          transition: `transform ${EXPAND_MS}ms ${EASE}`,
        }}
      >
        <Chevron dir="right" size={14} />
      </span>
    </button>
  );
}

/** Content that opens and closes under its header, its height animating. */
export function Disclosure(props: { open: boolean; children: JSX.Element }) {
  return (
    <div class="fm-disclosure" data-closed={props.open ? undefined : ''}>
      <div>{props.children}</div>
    </div>
  );
}

/** A row in the column: its name on the left and its value on the right,
 * with an inset hairline under it. `children` replaces the value. */
export function Row(props: {
  name: JSX.Element;
  value?: JSX.Element;
  /** Before the name: the row's pictogram. */
  leading?: JSX.Element;
  focused: boolean;
  /** Opens a fly-out or a further screen. */
  more?: boolean;
  /** Disabled, as SAVE with nothing to save. */
  idle?: boolean;
  anchor?: string;
  height?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      data-anchor={props.anchor}
      class={`flex w-full items-center gap-[8px] text-left whitespace-nowrap ${BODY}`}
      style={{
        height: `${props.height ?? ROW}px`,
        'padding-left': `${MARGIN}px`,
        'padding-right': `${MARGIN - 2}px`,
        color: props.idle ? 'rgb(255 255 255 / 0.35)' : props.focused ? '#fff' : DIM,
        ...TRACK(props.focused),
        'box-shadow': props.focused ? 'var(--m-focus-edge)' : HAIRLINE,
      }}
      onClick={() => props.onClick()}
    >
      {props.leading}
      <span class="min-w-0 grow truncate">{props.name}</span>
      <Show when={props.value !== undefined}>
        <span class={`shrink-0 ${VALUE}`} style={{ color: props.idle ? undefined : '#fff' }}>
          {props.value}
        </span>
      </Show>
      <Show when={props.more}>
        <Chevron dir="right" size={14} color={DIM} />
      </Show>
    </button>
  );
}
