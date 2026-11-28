import { For, Show, createEffect, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { reveal } from './column';
import { Check, Chevron } from './controls';
import { APPEAR, BODY, CAPTION, DIM, HEADER, RAISED } from './materials';

/** A pop-up row's height. */
const POP_ROW = 52;
/** A pop-up shows this many rows before it scrolls. */
const POP_ROWS = 7;

/** One choice in a picker: its name, and artwork drawn in the given ink. */
export interface PickerOption {
  label: string;
  art?: (ink: string) => JSX.Element;
  /** A screen past this choice, reached with RIGHT or a tap on its ›. */
  more?: () => void;
}

/** The choices of a pop-up: a check column for the choice in effect, then
 * each name with its artwork. The cursor is the focus plate and stripe. */
export function PickerList(props: {
  options: PickerOption[];
  /** The cursor. */
  at: number;
  /** The choice in effect, if it is one of the options. */
  current?: number;
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
  /* Longer lists scroll, fading at the edge that has more. */
  const scrolls = () => props.options.length > POP_ROWS;
  return (
    <div
      ref={(element) => (list = element)}
      class="relative grow overflow-y-auto overscroll-contain"
      style={{
        'scrollbar-width': 'none',
        ...(scrolls()
          ? {
              'mask-image':
                'linear-gradient(to bottom, transparent, #000 16px, #000 calc(100% - 16px), transparent)',
            }
          : {}),
      }}
    >
      <For each={props.options}>
        {(option, i) => (
          <button
            type="button"
            ref={(element) => (rows[i()] = element)}
            class={`flex w-full items-center gap-[8px] pr-[14px] pl-[8px] text-left whitespace-nowrap ${BODY}`}
            style={{
              height: `${POP_ROW}px`,
              color: props.at === i() ? '#fff' : 'rgb(255 255 255 / 0.85)',
              'box-shadow':
                [
                  props.at === i() ? 'var(--m-cursor-edge)' : '',
                  i() > 0 ? 'inset 0 1px 0 var(--m-separator)' : '',
                ]
                  .filter(Boolean)
                  .join(', ') || 'none',
              ...(props.at === i() ? { background: 'var(--m-cursor)' } : {}),
            }}
            onClick={() => props.choose(i())}
          >
            <span class="flex w-[24px] shrink-0 justify-center" style={{ color: 'var(--m-check)' }}>
              <Show when={props.current === i()}>
                <Check />
              </Show>
            </span>
            {option.art?.(props.at === i() ? '#fff' : DIM)}
            <span class="grow">{option.label}</span>
            <Show when={option.more}>
              {(more) => (
                <span
                  class="-mr-[10px] flex h-full items-center px-[10px]"
                  style={{ color: DIM }}
                  onClick={(e) => {
                    e.stopPropagation();
                    more()();
                  }}
                >
                  <Chevron dir="right" size={14} />
                </span>
              )}
            </Show>
          </button>
        )}
      </For>
    </div>
  );
}

/** The pop-up panel: the raised material, beside the column with its top
 * level with the row it comes from (raised only as far as it must be to stay
 * on screen), a ‹ pointing back at that row, and an optional title. It grows
 * in from that row. `height` is its content's height; taller content scrolls. */
export function FlyOut(props: {
  left: number;
  rowTop: number;
  height: number;
  title?: string;
  children: JSX.Element;
  screenHeight?: number;
}) {
  const MARGIN = 8;
  const screen = () => props.screenHeight ?? 480;
  const full = () => props.height + (props.title ? HEADER : 0);
  const height = () => Math.min(full(), screen() - 2 * MARGIN);
  const top = () => Math.max(MARGIN, Math.min(screen() - MARGIN - height(), props.rowTop));
  /* A panel wider than the room beside the column moves left over the
     column, as the camera's option list overlaps its row. */
  const [width, setWidth] = createSignal(0);
  const left = () => Math.max(MARGIN + 14, Math.min(props.left + 14, 720 - MARGIN - width()));
  /** The pointer sits level with the middle of the row it came from. */
  const pointer = () => Math.max(top() + 12, Math.min(top() + height() - 24, props.rowTop + 12));
  return (
    <>
      <span
        class="pointer-events-none absolute z-30 flex"
        style={{ left: `${left() - 16}px`, top: `${pointer()}px`, color: '#fff', ...APPEAR }}
      >
        <Chevron dir="left" size={16} />
      </span>
      <div
        ref={(panel: HTMLDivElement) => requestAnimationFrame(() => setWidth(panel.offsetWidth))}
        class="absolute z-30 flex w-max min-w-[220px] flex-col overflow-hidden p-[1px]"
        style={{
          left: `${left()}px`,
          top: `${top()}px`,
          height: `${height()}px`,
          'max-width': `${720 - 2 * MARGIN}px`,
          ...RAISED,
          ...APPEAR,
          'transform-origin': `0 ${pointer() - top() + 8}px`,
        }}
      >
        <Show when={props.title}>
          <div
            class={`flex shrink-0 items-center px-[16px] font-semibold ${CAPTION}`}
            style={{
              height: `${HEADER}px`,
              color: DIM,
              'box-shadow': 'inset 0 -1px 0 var(--m-separator)',
            }}
          >
            {props.title}
          </div>
        </Show>
        {props.children}
      </div>
    </>
  );
}

/** A pop-up from the element marked `data-anchor={anchor}` on the same
 * screen: its top level with that element's top. */
export function AnchoredFlyOut(props: {
  anchor: string;
  left: number;
  height: number;
  title?: string;
  children: JSX.Element;
}) {
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
          <FlyOut left={props.left} rowTop={rowTop()} height={props.height} title={props.title}>
            {props.children}
          </FlyOut>
        )}
      </Show>
    </>
  );
}

/** A pop-up list's height for `count` rows, inside its 1-pixel border. */
export const flyOutListHeight = (count: number) => Math.min(count, POP_ROWS) * POP_ROW + 2;

/** The cursor after a key in a picker of `count` options; undefined for keys a picker does not take. */
export function pickerStep(
  at: number,
  count: number,
  key: string,
  columns = 1,
): number | undefined {
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
