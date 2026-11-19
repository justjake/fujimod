import { For, Show, createEffect, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { ACCENT, SMALL, TRACK } from './materials';

/* The editors' row controls: the choice strip and the slider card, shared by
   the custom settings editor and the film editor. Both take touch (a tap
   chooses, a sideways drag slides, a swipe coasts and settles) and keys
   through their owner's state. */

export const signed = (n: number) => (n > 0 ? `+${n}` : n < 0 ? `−${-n}` : '0');

/** Focal UI's chevron: a stroked, round-capped angle, never a text glyph. */
export function Chevron(props: { dir: 'down' | 'left' | 'right'; size?: number; color?: string }) {
  const d = { down: 'M2 3l6 6 6-6', left: 'M10 2L4 8l6 6', right: 'M6 2l6 6-6 6' }[props.dir];
  const size = props.size ?? 16;
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" class="shrink-0" style={{ color: props.color ?? 'currentColor' }}>
      <path d={d} fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  );
}

/* A choice row, the recipe column's strip in the slider card's frame: the name
   on the top line, the choices as words in the recessed strip below. The
   chosen word is a white plate with black ink; the others fade with their
   distance from it. The strip slides to keep the chosen word in the middle,
   with a chevron at an end that has more past it. A tap on a word chooses it;
   LEFT, RIGHT and the dial step. */
/** A choice: a word, or artwork drawn in the given ink. */
export type ChoiceItem = string | { name: string; art: (ink: string) => JSX.Element };

export function Choice(props: {
  /** The top line's name; without one the strip fills the card. */
  name?: string;
  items: readonly ChoiceItem[];
  value: number;
  width: number;
  height?: number;
  focused: boolean;
  changed?: boolean;
  pick: (i: number) => void;
  onFocus?: () => void;
}) {
  let track: HTMLDivElement | undefined;
  const [offset, setOffset] = createSignal(0);
  const [ends, setEnds] = createSignal<[boolean, boolean]>([false, false]);
  createEffect(
    () => [props.value, props.width, props.items] as const,
    ([value, width]) => {
      requestAnimationFrame(() => {
        const cells = track ? (Array.from(track.children) as HTMLElement[]) : [];
        const chosen = cells[value];
        if (!track || !chosen) return;
        const max = Math.max(0, track.scrollWidth - width);
        const next = Math.max(0, Math.min(max, chosen.offsetLeft + chosen.offsetWidth / 2 - width / 2));
        setOffset(next);
        setEnds([next > 0, next < max]);
      });
    },
  );
  return (
    <div class="relative shrink-0 overflow-hidden select-none" style={{ width: `${props.width}px`, height: `${props.height ?? 76}px`, ...TRACK(props.focused) }} onPointerDown={() => props.onFocus?.()}>
      <Show when={props.name !== undefined}>
        <span class="absolute top-[7px] left-[12px] flex items-center gap-[6px] text-[17px] leading-[20px] tracking-[0.06em]" style={{ color: props.focused ? '#fff' : 'rgb(255 255 255 / 0.7)' }}>
          {props.name}
          <Show when={props.changed}>
            <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
          </Show>
        </span>
      </Show>
      <div class="absolute inset-x-0 bottom-0 overflow-hidden" style={{ top: props.name === undefined ? '0' : 'auto', height: props.name === undefined ? 'auto' : '44px', background: 'var(--m-well)', 'box-shadow': 'var(--m-well-edge)' }}>
        <div
          ref={(element) => (track = element)}
          class="absolute inset-y-0 left-0 flex items-stretch gap-[4px] px-[6px] py-[5px] transition-transform duration-150 ease-out"
          style={{ transform: `translateX(${-offset()}px)` }}
        >
          <For each={[...props.items]}>
            {(item, i) => {
              const chosen = () => i() === props.value;
              const fade = () => Math.max(0.35, 0.85 - Math.abs(i() - props.value) * 0.15);
              const ink = () => (chosen() ? '#000' : `rgb(255 255 255 / ${fade()})`);
              return (
                <button
                  type="button"
                  class={`relative flex shrink-0 items-center justify-center rounded-[9px] font-medium whitespace-nowrap ${SMALL} ${typeof item === 'string' ? 'px-[12px]' : 'w-[56px]'}`}
                  style={chosen() ? { background: '#fff', color: ink() } : { color: ink() }}
                  onClick={() => props.pick(i())}
                >
                  {typeof item === 'string' ? item : item.art(ink())}
                </button>
              );
            }}
          </For>
        </div>
        <For each={[0, 1] as const}>
          {(side) => (
            <Show when={ends()[side]}>
              <span class="pointer-events-none absolute inset-y-0 flex items-center px-[3px] text-white/80" style={{ [side ? 'right' : 'left']: '0', background: `linear-gradient(to ${side ? 'left' : 'right'}, rgb(0 0 0 / 0.55), transparent)` }}>
                <Chevron dir={side ? 'right' : 'left'} size={14} />
              </span>
            </Show>
          )}
        </For>
      </div>
      {/* Without a name, changed is a dot at the card's left edge. */}
      <Show when={props.name === undefined && props.changed}>
        <span class="absolute top-1/2 left-[3px] z-10 size-[6px] -translate-y-1/2 rounded-full" style={{ background: ACCENT }} />
      </Show>
    </div>
  );
}

/* A slider row, as the camera's command dials and the recipe column's rulers
   behave: the whole row is the track; the name and value sit on its top line
   and a scale of ticks runs across its full width under a fixed center mark.
   A sideways drag moves the scale with the finger, one to one and without
   snapping while held, the value changing as each tick crosses the mark and
   the picture following; let go and it coasts and settles on a step. A
   vertical drag scrolls the sidebar instead. A tap left or right of the mark
   steps once. While held, the value grows and the mark and the ticks near it
   light up. */
export function Dial(props: {
  /** The top line's name and value; without one the scale fills the card. */
  name?: string;
  height?: number;
  /** The value labeled as zero, the scale's origin; 0 by default. */
  zero?: number;
  /** A step's label; signed numbers by default. */
  format?: (value: number) => string;
  value: number;
  min: number;
  max: number;
  step: number;
  width: number;
  focused: boolean;
  changed: boolean;
  track?: string;
  set: (value: number) => void;
  onFocus?: () => void;
}) {
  const count = () => Math.round((props.max - props.min) / props.step) + 1;
  /* Ticks every step, as far apart as the step count allows; a labeled major
     tick every few steps, counted from zero. */
  const SPACING = props.name === undefined ? 38 : count() <= 11 ? 30 : count() <= 21 ? 20 : 15;
  const MAJOR = props.name === undefined ? 1 : SPACING >= 30 ? 2 : SPACING >= 20 ? 4 : 5;
  const label = (v: number) => (props.format ?? signed)(v);
  const indexOf = (v: number) => (v - props.min) / props.step;
  const zero = () => indexOf(props.zero ?? 0);
  /** The scale's position in steps while held or coasting; settled otherwise. */
  const [pos, setPos] = createSignal<number | null>(null);
  const shown = () => pos() ?? indexOf(props.value);
  const active = () => pos() !== null;
  const color = () => '#fff';
  let gesture: { x: number; y: number; start: number; axis: 'x' | 'y' | null; samples: [number, number][] } | undefined;
  let frame = 0;
  const stop = () => cancelAnimationFrame(frame);
  const commit = (p: number) => {
    const i = Math.max(0, Math.min(count() - 1, Math.round(p)));
    const v = props.min + i * props.step;
    if (v !== props.value) props.set(v);
  };
  /* Past either end the scale gives a third as much, as a rubber band. */
  const band = (p: number) => (p < 0 ? p / 3 : p > count() - 1 ? count() - 1 + (p - count() + 1) / 3 : p);
  const settle = () => {
    const from = pos() ?? indexOf(props.value);
    const to = Math.max(0, Math.min(count() - 1, Math.round(from)));
    const t0 = performance.now();
    const tick = (now: number) => {
      const t = Math.min(1, (now - t0) / 200);
      setPos(from + (to - from) * (1 - (1 - t) ** 3));
      if (t < 1) frame = requestAnimationFrame(tick);
      else setPos(null);
    };
    commit(to);
    frame = requestAnimationFrame(tick);
  };
  const coast = (velocity: number) => {
    let v = velocity;
    let last = performance.now();
    const tick = (now: number) => {
      const dt = now - last;
      last = now;
      v *= 0.94 ** (dt / 16);
      const next = (pos() ?? 0) + (v * dt) / SPACING;
      const clamped = Math.max(-0.4, Math.min(count() - 0.6, next));
      setPos(clamped);
      commit(clamped);
      if (Math.abs(v) < 0.08 || clamped !== next) settle();
      else frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
  };
  const scaleOf = (el: Element) => (el as HTMLElement).offsetWidth / el.getBoundingClientRect().width;
  return (
    <div
      class="relative shrink-0 overflow-hidden select-none"
      style={{ width: `${props.width}px`, height: `${props.height ?? 76}px`, 'touch-action': 'pan-y', ...TRACK(props.focused || active()) }}
      onPointerDown={(e) => {
        stop();
        props.onFocus?.();
        gesture = { x: e.clientX, y: e.clientY, start: pos() ?? indexOf(props.value), axis: null, samples: [[performance.now(), e.clientX]] };
      }}
      onPointerMove={(e) => {
        if (!gesture) return;
        const k = scaleOf(e.currentTarget);
        const dx = (e.clientX - gesture.x) * k;
        const dy = (e.clientY - gesture.y) * k;
        if (!gesture.axis) {
          if (Math.abs(dx) > 8 && Math.abs(dx) > Math.abs(dy)) {
            gesture.axis = 'x';
            e.currentTarget.setPointerCapture(e.pointerId);
          } else if (Math.abs(dy) > 8) gesture.axis = 'y';
        }
        if (gesture.axis !== 'x') return;
        gesture.samples = [...gesture.samples.slice(-4), [performance.now(), e.clientX]];
        const p = band(gesture.start - dx / SPACING);
        setPos(p);
        commit(p);
      }}
      onPointerUp={(e) => {
        const g = gesture;
        gesture = undefined;
        if (!g) return;
        if (g.axis === 'x') {
          const [[t0, x0], [t1, x1]] = [g.samples[0], g.samples[g.samples.length - 1]];
          const velocity = t1 > t0 ? (-(x1 - x0) * scaleOf(e.currentTarget)) / (t1 - t0) : 0;
          if (Math.abs(velocity) > 0.3) coast(velocity);
          else settle();
        } else if (!g.axis) {
          const box = e.currentTarget.getBoundingClientRect();
          const right = e.clientX > box.left + box.width / 2;
          const i = Math.max(0, Math.min(count() - 1, Math.round(indexOf(props.value)) + (right ? 1 : -1)));
          props.set(props.min + i * props.step);
        }
      }}
      onPointerCancel={() => {
        gesture = undefined;
        if (active()) settle();
      }}
    >
      <Show when={props.name !== undefined}>
        <span class="absolute top-[7px] left-[12px] flex items-center gap-[6px] text-[17px] leading-[20px] tracking-[0.06em]" style={{ color: props.focused || active() ? '#fff' : 'rgb(255 255 255 / 0.7)' }}>
          {props.name}
          {/* Changed from the film as opened: the same dot the rail uses. */}
          <Show when={props.changed}>
            <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
          </Show>
        </span>
        <span class="absolute top-[4px] right-[12px] origin-right font-semibold tabular-nums transition-transform duration-100" style={{ color: color(), 'font-size': '22px', 'line-height': '26px', transform: active() ? 'scale(1.2)' : 'none' }}>
          {label(props.value)}
        </span>
      </Show>
      {/* The scale: a recessed strip along the bottom, the track's colors on
          its bottom edge, labels over the major ticks, the mark at the center. */}
      <div class="absolute inset-x-0 bottom-0" style={{ top: props.name === undefined ? '0' : 'auto', height: props.name === undefined ? 'auto' : '44px', background: 'var(--m-well)', 'box-shadow': 'var(--m-well-edge)' }}>
        <div class="absolute inset-0" style={{ 'mask-image': 'linear-gradient(to right, transparent, #000 32px, #000 calc(100% - 32px), transparent)' }}>
          <div class="absolute inset-y-0 left-0" style={{ transform: `translateX(${props.width / 2 - shown() * SPACING}px)`, transition: active() ? 'none' : 'transform 160ms ease-out' }}>
            <For each={Array.from({ length: count() }, (_, i) => i)}>
              {(i) => {
                const major = () => (i - Math.round(zero())) % MAJOR === 0;
                const near = () => (active() ? Math.max(0, 1 - Math.abs(i - shown()) / 2.5) : 0);
                return (
                  <>
                    <Show when={major()}>
                      <span class="absolute top-[3px] -translate-x-1/2 text-[15px] leading-none font-medium tabular-nums" style={{ left: `${i * SPACING}px`, color: `rgb(255 255 255 / ${0.55 + near() * 0.45})` }}>
                        {label(props.min + i * props.step)}
                      </span>
                    </Show>
                    <span
                      class="fm-round absolute bottom-[6px] w-[3px] -translate-x-1/2 rounded-[1.5px]"
                      style={{ left: `${i * SPACING}px`, height: `${major() ? 20 : 12}px`, background: `rgb(255 255 255 / ${(major() ? 0.75 : 0.45) + near() * 0.25})` }}
                    />
                  </>
                );
              }}
            </For>
          </div>
        </div>
        <Show when={props.track}>{(track) => <div class="absolute inset-x-0 bottom-0 h-[4px]" style={{ background: track() }} />}</Show>
        <span class="fm-round absolute bottom-[4px] w-[4px] -translate-x-1/2 rounded-[2px] transition-[height] duration-100" style={{ left: `${props.width / 2}px`, height: active() ? '30px' : '24px', background: 'var(--m-mark)', 'box-shadow': '0 0 0 1px rgb(0 0 0 / 0.55)' }} />
      </div>
      {/* Without a name, changed is a dot at the card's left edge. */}
      <Show when={props.name === undefined && props.changed}>
        <span class="absolute top-1/2 left-[3px] z-10 size-[6px] -translate-y-1/2 rounded-full" style={{ background: ACCENT }} />
      </Show>
    </div>
  );
}

