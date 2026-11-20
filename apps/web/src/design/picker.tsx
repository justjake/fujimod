import { For, Show, createEffect, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { reveal } from './column';
import { Chevron } from './controls';
import { ACCENT, SELECTED, SMALL } from './materials';

/** One choice in a picker: its name, and artwork drawn in the given ink. */
export interface PickerOption {
  label: string;
  art?: (ink: string) => JSX.Element;
  /** A screen past this choice, reached with RIGHT or a tap on its ›. */
  more?: () => void;
}

/** The choices of a picker, in whatever holds them (a sheet, or a row's
 * fly-out): a list of names (with their artwork) or, with `columns`, a grid of
 * artwork. The cursor is the selected plate; the current choice is marked. */
export function PickerList(props: {
  options: PickerOption[];
  /** The cursor. */
  at: number;
  /** The choice in effect, if it is one of the options. */
  current?: number;
  columns?: number;
  choose: (index: number) => void;
}) {
  let list: HTMLDivElement | undefined;
  const rows: HTMLElement[] = [];
  let opened = false;
  createEffect(
    () => props.at,
    (at) => {
      const row = rows[at];
      if (list && row) reveal(list, row.offsetTop, row.offsetTop + row.offsetHeight, 8, opened);
      opened = true;
    },
  );
  const ink = (_i: number) => '#fff';
  return (
      <div
        ref={(element) => (list = element)}
        class={`relative grow overflow-y-auto overscroll-contain ${props.columns ? 'grid content-start gap-[4px]' : ''}`}
        style={{ 'scrollbar-width': 'none', ...(props.columns ? { 'grid-template-columns': `repeat(${props.columns}, 1fr)` } : {}) }}
      >
        <For each={props.options}>
          {(option, i) => (
            <button
              type="button"
              ref={(element) => (rows[i()] = element)}
              class={
                props.columns
                  ? 'relative flex aspect-square items-center justify-center rounded-[10px]'
                  : `flex h-[44px] w-full items-center gap-[10px] rounded-[10px] pr-[12px] pl-[6px] text-left whitespace-nowrap ${SMALL}`
              }
              style={{
                color: '#fff',
                ...(props.at === i() ? SELECTED : {}),
                ...(props.columns || i() === 0 || props.at === i() || props.at === i() - 1 ? {} : { 'box-shadow': 'inset 0 1px 0 var(--m-separator)', 'border-radius': '0' }),
              }}
              onClick={() => props.choose(i())}
            >
              <Show when={!props.columns}>
                <span class="w-[20px] shrink-0 text-center" style={{ color: 'var(--m-check)' }}>
                  {props.current === i() ? '✓' : ''}
                </span>
              </Show>
              {option.art?.(ink(i()))}
              <Show when={!props.columns}>
                <span class="grow">{option.label}</span>
              </Show>
              <Show when={props.columns && props.current === i()}>
                <span class="absolute top-[3px] right-[3px] size-[8px] rounded-full" style={{ background: ACCENT }} />
              </Show>
              <Show when={option.more}>
                {(more) => (
                  <span
                    class="-mr-[6px] flex h-full items-center px-[6px]"
                    onClick={(e) => {
                      e.stopPropagation();
                      more()();
                    }}
                  >
                    <Chevron dir="right" size={16} />
                  </span>
                )}
              </Show>
            </button>
          )}
        </For>
      </div>
  );
}

/** What comes out of a row: a panel flush against the column, as wide as its
 * content, its top level with the top of the row it comes from (raised only
 * as far as it must be to stay on screen), in the look's fly-out material.
 * `height` is the content's height; content taller than the screen scrolls. */
export function FlyOut(props: { left: number; rowTop: number; height: number; children: JSX.Element; screenHeight?: number }) {
  const MARGIN = 8;
  const screen = () => props.screenHeight ?? 480;
  const height = () => Math.min(props.height, screen() - 2 * MARGIN);
  const top = () => Math.max(MARGIN, Math.min(screen() - MARGIN - height(), props.rowTop));
  return (
    <div
      class="absolute flex w-max min-w-[220px] flex-col overflow-hidden p-[6px]"
      style={{
        left: `${props.left}px`,
        top: `${top()}px`,
        height: `${height()}px`,
        'max-width': `${720 - props.left - MARGIN}px`,
        background: 'var(--m-popup)',
        'box-shadow': 'var(--m-popup-edge)',
        'border-radius': '0 16px 16px 0',
        color: '#fff',
      }}
    >
      {props.children}
    </div>
  );
}

/** A fly-out from the element marked `data-anchor={anchor}` on the same
 * screen: its top level with that element's top. */
export function AnchoredFlyOut(props: { anchor: string; left: number; height: number; children: JSX.Element }) {
  const [top, setTop] = createSignal<number>();
  const measure = (probe: HTMLElement) =>
    requestAnimationFrame(() => {
      const screen = probe.closest('.fm-screen');
      const anchor = screen?.querySelector(`[data-anchor="${props.anchor}"]`);
      if (!screen || !anchor) return setTop(8);
      const box = screen.getBoundingClientRect();
      setTop((anchor.getBoundingClientRect().top - box.top) * (720 / box.width));
    });
  return (
    <>
      <span ref={measure} class="hidden" />
      <Show when={top()}>
        {(rowTop) => (
          <FlyOut left={props.left} rowTop={rowTop()} height={props.height}>
            {props.children}
          </FlyOut>
        )}
      </Show>
    </>
  );
}

/** A fly-out list's height for `count` rows. */
export const flyOutListHeight = (count: number) => count * 44 + 12;

/** The cursor after a key in a picker of `count` options; undefined for keys a picker does not take. */
export function pickerStep(at: number, count: number, key: string, columns = 1): number | undefined {
  const step = (
    {
      up: -columns,
      down: columns,
      'dial-': -1,
      'dial+': 1,
      ...(columns > 1 ? { left: -1, right: 1 } : {}),
    } as Record<string, number>
  )[key];
  return step === undefined ? undefined : Math.max(0, Math.min(count - 1, at + step));
}
