import { Show } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { ACCENT, COLUMN, SMALL, TEXT, TRACK } from './materials';

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
export function reveal(scroller: HTMLElement, top: number, bottom: number, margin: number, smooth: boolean) {
  const behavior = smooth ? 'smooth' : 'auto';
  if (top < scroller.scrollTop + margin / 2) scroller.scrollTo({ top: top - margin, behavior });
  else if (bottom > scroller.scrollTop + scroller.clientHeight - margin / 2)
    scroller.scrollTo({ top: bottom - scroller.clientHeight + margin, behavior });
}

/** A titled group of rows, divided from the group above. */
export function Section(props: {
  ref?: (element: HTMLElement) => void;
  title?: string;
  /** Beside the title on its right, such as a chip. */
  accessory?: JSX.Element;
  first?: boolean;
  /** A long title: a size smaller, and free to wrap. */
  long?: boolean;
  children: JSX.Element;
}) {
  return (
    <section
      ref={(element) => props.ref?.(element)}
      class={props.first ? 'pt-[8px] pb-[14px]' : 'pt-[10px] pb-[14px]'}
      style={props.first ? {} : { 'box-shadow': 'var(--m-divider)' }}
    >
      <Show when={props.title}>
        <div class="mb-[6px] flex h-[44px] items-center gap-[8px] px-[6px]">
          <span class={`grow font-semibold tracking-[0.02em] ${props.long ? SMALL : `whitespace-nowrap ${TEXT}`}`}>{props.title}</span>
          {props.accessory}
        </div>
      </Show>
      {props.children}
    </section>
  );
}

/** An action in the column: its label on a track, the focused one lit. */
export function Button(props: {
  label: string;
  focused: boolean;
  /** Filled in the accent, for an action that is done. */
  accent?: boolean;
  /** Nothing to do yet. */
  idle?: boolean;
  class?: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      class={`flex h-[47px] items-center justify-center px-[10px] font-semibold whitespace-nowrap ${SMALL} ${props.class ?? ''}`}
      style={{
        ...TRACK(props.focused),
        ...(props.accent ? { background: ACCENT } : {}),
        ...(props.idle ? { color: 'rgb(255 255 255 / 0.35)' } : {}),
      }}
      onClick={() => props.onClick()}
    >
      {props.label}
    </button>
  );
}
