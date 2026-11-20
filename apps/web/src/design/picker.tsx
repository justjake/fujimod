import { For, Show, createEffect } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { reveal } from './column';
import { Chevron } from './controls';
import { ACCENT, GLASS, SELECTED, SMALL } from './materials';

/** One choice in a picker: its name, and artwork drawn in the given ink. */
export interface PickerOption {
  label: string;
  art?: (ink: string) => JSX.Element;
  /** A screen past this choice, reached with RIGHT or a tap on its ›. */
  more?: () => void;
}

/** A sheet over the picture choosing one of many, under its title. UP, DOWN
 * and the dial move the cursor, OK chooses, BACK closes. */
export function Picker(props: {
  title: string;
  options: PickerOption[];
  at: number;
  current?: number;
  columns?: number;
  choose: (index: number) => void;
  style?: JSX.CSSProperties;
}) {
  return (
    <div class="absolute top-[8px] bottom-[8px] flex flex-col overflow-hidden rounded-[16px] p-[8px]" style={{ ...GLASS, background: 'var(--m-panel)', ...props.style }}>
      <div class={`px-[8px] pt-[6px] pb-[10px] font-semibold ${SMALL}`}>{props.title}</div>
      <PickerList options={props.options} at={props.at} current={props.current} columns={props.columns} choose={props.choose} />
    </div>
  );
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
                  : `flex h-[44px] w-full items-center gap-[10px] rounded-[10px] px-[10px] text-left ${SMALL}`
              }
              style={props.at === i() ? { ...SELECTED, color: '#fff' } : { color: '#fff' }}
              onClick={() => props.choose(i())}
            >
              {option.art?.(ink(i()))}
              <Show when={!props.columns}>
                <span class="grow truncate">{option.label}</span>
              </Show>
              <Show when={props.current === i()}>
                {props.columns ? (
                  <span class="absolute top-[3px] right-[3px] size-[8px] rounded-full" style={{ background: ACCENT }} />
                ) : (
                  <span>✓</span>
                )}
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
