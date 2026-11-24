import { For } from 'solid-js';

import { DETAIL, LABEL, TEXT } from './materials';

/* A number pad down the right edge of the screen, in the fly-outs' solid
   panel, for typing a value instead of stepping to it. The D-pad moves
   between keys, OK presses one, BACK puts the value back. */

/** The pad's keys in order; the last row is delete and OK (two cells). */
export const PAD_KEYS = ['7', '8', '9', '4', '5', '6', '1', '2', '3', '±', '0', '.', '⌫', 'OK'] as const;
export type PadKey = (typeof PAD_KEYS)[number];
export const PAD_WIDTH = 276;

/** Each key's row and first and last column. */
const PLACE = PAD_KEYS.map((_, i) => (i < 12 ? { row: Math.floor(i / 3), from: i % 3, to: i % 3 } : i === 12 ? { row: 4, from: 0, to: 0 } : { row: 4, from: 1, to: 2 }));

/** The key the D-pad reaches from `at`, or `at` at an edge. */
export function padStep(at: number, key: 'up' | 'down' | 'left' | 'right'): number {
  const here = PLACE[at];
  const column = key === 'left' ? here.from - 1 : key === 'right' ? here.to + 1 : here.from;
  const row = here.row + (key === 'up' ? -1 : key === 'down' ? 1 : 0);
  const next = PLACE.findIndex((p) => p.row === row && p.from <= column && column <= p.to);
  return next < 0 ? at : next;
}

/** Typed text after pressing `key`; `fresh` text is replaced by the first digit. */
export function padType(text: string, fresh: boolean, key: PadKey, decimals: boolean): string {
  const base = fresh ? '' : text;
  if (key === '⌫') return fresh ? '' : base.slice(0, -1);
  if (key === '±') return text.startsWith('−') ? text.slice(1) : `−${text}`;
  if (key === '.') return !decimals || base.includes('.') ? text : `${base === '' || base === '−' ? `${base}0` : base}.`;
  if (key === 'OK') return text;
  return base.length >= 7 ? text : base + key;
}

/** The typed text as a number, or none while it is not one yet. */
export function padValue(text: string): number | undefined {
  const v = Number(text.replace('−', '-'));
  return text === '' || text === '−' || Number.isNaN(v) ? undefined : v;
}

export function NumberPad(props: {
  name: string;
  text: string;
  /** The range, as the pad shows it under the value. */
  range: string;
  decimals: boolean;
  at: number;
  press: (key: PadKey, at: number) => void;
}) {
  return (
    <div
      class="absolute inset-y-0 right-0 flex flex-col gap-[6px] p-[8px]"
      style={{ width: `${PAD_WIDTH}px`, background: 'var(--m-popup)', 'box-shadow': 'var(--m-popup-edge)' }}
    >
      <div class="flex flex-col gap-[6px] px-[6px] pt-[2px] pb-[4px]">
        <span class={`font-medium text-white/70 ${TEXT}`}>{props.name}</span>
        <span class={`h-[30px] text-right font-semibold tabular-nums ${LABEL}`}>
          {props.text}
          <span class="ml-[2px] inline-block h-[28px] w-[3px] translate-y-[3px] bg-white" />
        </span>
        <span class={`text-right text-white/50 tabular-nums ${DETAIL}`}>{props.range}</span>
      </div>
      <div class="grid grow grid-cols-3 grid-rows-5 gap-[6px]">
        <For each={[...PAD_KEYS]}>
          {(key, i) => {
            const off = () => key === '.' && !props.decimals;
            return (
              <button
                type="button"
                class={`flex items-center justify-center rounded-[10px] font-medium ${key === 'OK' ? 'col-span-2' : ''} ${TEXT}`}
                style={{
                  background: key === 'OK' ? 'var(--m-confirm)' : key === '⌫' || key === '±' || key === '.' ? 'var(--m-key-special)' : 'var(--m-key)',
                  'box-shadow': props.at === i() ? 'var(--m-focus-edge)' : 'var(--m-key-edge)',
                  color: off() ? 'rgb(255 255 255 / 0.25)' : '#fff',
                }}
                onClick={() => !off() && props.press(key, i())}
              >
                {key}
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}
