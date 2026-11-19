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

/* The dial gesture, shared by the choice strip and the slider: a sideways
   drag moves the position with the finger, one to one, the value changing
   as each step passes the middle; let go and it coasts and settles on a
   step. Past either end it gives a third as much, as a rubber band. A
   vertical drag scrolls the column instead. */
function useDialGesture(o: {
  count: () => number;
  /** The settled position: the current value's step. */
  index: () => number;
  /** Pixels per step. */
  spacing: () => number;
  commit: (step: number) => void;
  onFocus?: () => void;
  /** A press without a drag, at a client x. */
  tap?: (x: number, box: DOMRect) => void;
}) {
  const [pos, setPos] = createSignal<number | null>(null);
  const shown = () => pos() ?? o.index();
  const active = () => pos() !== null;
  let gesture: { x: number; y: number; start: number; axis: 'x' | 'y' | null; samples: [number, number][] } | undefined;
  let dragged = false;
  let frame = 0;
  const stop = () => cancelAnimationFrame(frame);
  const commit = (p: number) => {
    const i = Math.max(0, Math.min(o.count() - 1, Math.round(p)));
    if (i !== Math.round(o.index())) o.commit(i);
  };
  const band = (p: number) => (p < 0 ? p / 3 : p > o.count() - 1 ? o.count() - 1 + (p - o.count() + 1) / 3 : p);
  const settle = () => {
    const from = pos() ?? o.index();
    const to = Math.max(0, Math.min(o.count() - 1, Math.round(from)));
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
      const next = (pos() ?? 0) + (v * dt) / o.spacing();
      const clamped = Math.max(-0.4, Math.min(o.count() - 0.6, next));
      setPos(clamped);
      commit(clamped);
      if (Math.abs(v) < 0.08 || clamped !== next) settle();
      else frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
  };
  const scaleOf = (el: Element) => (el as HTMLElement).offsetWidth / el.getBoundingClientRect().width;
  const handlers = {
    onPointerDown: (e: PointerEvent & { currentTarget: HTMLElement }) => {
      stop();
      dragged = false;
      o.onFocus?.();
      gesture = { x: e.clientX, y: e.clientY, start: pos() ?? o.index(), axis: null, samples: [[performance.now(), e.clientX]] };
    },
    onPointerMove: (e: PointerEvent & { currentTarget: HTMLElement }) => {
      if (!gesture) return;
      const k = scaleOf(e.currentTarget);
      const dx = (e.clientX - gesture.x) * k;
      const dy = (e.clientY - gesture.y) * k;
      if (!gesture.axis) {
        if (Math.abs(dx) > 8 && Math.abs(dx) > Math.abs(dy)) {
          gesture.axis = 'x';
          dragged = true;
          e.currentTarget.setPointerCapture(e.pointerId);
        } else if (Math.abs(dy) > 8) gesture.axis = 'y';
      }
      if (gesture.axis !== 'x') return;
      gesture.samples = [...gesture.samples.slice(-4), [performance.now(), e.clientX]];
      const p = band(gesture.start - dx / o.spacing());
      setPos(p);
      commit(p);
    },
    onPointerUp: (e: PointerEvent & { currentTarget: HTMLElement }) => {
      const g = gesture;
      gesture = undefined;
      if (!g) return;
      if (g.axis === 'x') {
        const [[t0, x0], [t1, x1]] = [g.samples[0], g.samples[g.samples.length - 1]];
        const velocity = t1 > t0 ? (-(x1 - x0) * scaleOf(e.currentTarget)) / (t1 - t0) : 0;
        if (Math.abs(velocity) > 0.3) coast(velocity);
        else settle();
      } else if (!g.axis) o.tap?.(e.clientX, e.currentTarget.getBoundingClientRect());
    },
    onPointerCancel: () => {
      gesture = undefined;
      if (active()) settle();
    },
  };
  return { shown, active, handlers, dragged: () => dragged };
}

/* A choice row: the slider's dial for a list of choices. The choices sit
   as words or artwork in the recessed strip; the chosen one is a white
   plate with black ink, the others fading with their distance from it. The
   strip keeps the chosen cell in the middle, with a chevron at an end that
   has more past it. A thumb sliding sideways moves the strip with it and
   chooses each cell as it reaches the middle; a swipe coasts and settles. A
   tap on a cell chooses it; LEFT, RIGHT and the dial step. */
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
  /** Each cell's center along the strip, measured once laid out. */
  const [centers, setCenters] = createSignal<number[]>([]);
  const [length, setLength] = createSignal(0);
  createEffect(
    () => [props.width, props.items] as const,
    () => {
      requestAnimationFrame(() => {
        const cells = track ? (Array.from(track.children) as HTMLElement[]) : [];
        setCenters(cells.map((c) => c.offsetLeft + c.offsetWidth / 2));
        setLength(track?.scrollWidth ?? 0);
      });
    },
  );
  const dial = useDialGesture({
    count: () => props.items.length,
    index: () => props.value,
    spacing: () => (centers().length > 1 ? (centers()[centers().length - 1] - centers()[0]) / (centers().length - 1) : 60),
    commit: (i) => props.pick(i),
    onFocus: () => props.onFocus?.(),
  });
  /** The strip's center at a fractional cell, past the ends by one spacing per cell. */
  const centerAt = (p: number) => {
    const c = centers();
    if (c.length === 0) return 0;
    const last = c.length - 1;
    const step = last > 0 ? (c[last] - c[0]) / last : 0;
    if (p <= 0) return c[0] + p * step;
    if (p >= last) return c[last] + (p - last) * step;
    const i = Math.floor(p);
    return c[i] + (c[i + 1] - c[i]) * (p - i);
  };
  const max = () => Math.max(0, length() - props.width);
  const offset = () => {
    const raw = centerAt(dial.shown()) - props.width / 2;
    return dial.active() ? raw : Math.max(0, Math.min(max(), raw));
  };
  const ends = (): [boolean, boolean] => [offset() > 1, offset() < max() - 1];
  const chosen = () => Math.max(0, Math.min(props.items.length - 1, Math.round(dial.shown())));
  return (
    <div
      class="relative shrink-0 overflow-hidden select-none"
      style={{ width: `${props.width}px`, height: `${props.height ?? 76}px`, 'touch-action': 'pan-y', ...TRACK(props.focused || dial.active()) }}
      {...dial.handlers}
    >
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
          class="absolute inset-y-0 left-0 flex items-stretch gap-[4px] px-[6px] py-[5px]"
          style={{ transform: `translateX(${-offset()}px)`, transition: dial.active() ? 'none' : 'transform 150ms ease-out' }}
        >
          <For each={[...props.items]}>
            {(item, i) => {
              const on = () => i() === chosen();
              const fade = () => Math.max(0.35, 0.85 - Math.abs(i() - dial.shown()) * 0.15);
              const ink = () => (on() ? '#000' : `rgb(255 255 255 / ${fade()})`);
              return (
                <button
                  type="button"
                  class={`relative flex shrink-0 items-center justify-center rounded-[9px] font-medium whitespace-nowrap ${SMALL} ${typeof item === 'string' ? 'px-[12px]' : 'w-[56px]'}`}
                  style={on() ? { background: '#fff', color: ink() } : { color: ink() }}
                  onClick={() => {
                    if (!dial.dragged()) props.pick(i());
                  }}
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

/* A slider row, as the camera's command dials behave: the whole row is the
   track; the name and value sit on its top line and a scale of ticks runs
   across its full width under a fixed center mark, moved by the dial
   gesture above. A tap left or right of the mark steps once. While held,
   the value grows and the mark and the ticks near it light up. */
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
  const dial = useDialGesture({
    count,
    index: () => indexOf(props.value),
    spacing: () => SPACING,
    commit: (i) => props.set(props.min + i * props.step),
    onFocus: () => props.onFocus?.(),
    tap: (x, box) => {
      const i = Math.max(0, Math.min(count() - 1, Math.round(indexOf(props.value)) + (x > box.left + box.width / 2 ? 1 : -1)));
      props.set(props.min + i * props.step);
    },
  });
  const shown = dial.shown;
  const active = dial.active;
  return (
    <div
      class="relative shrink-0 overflow-hidden select-none"
      style={{ width: `${props.width}px`, height: `${props.height ?? 76}px`, 'touch-action': 'pan-y', ...TRACK(props.focused || active()) }}
      {...dial.handlers}
    >
      <Show when={props.name !== undefined}>
        <span class="absolute top-[7px] left-[12px] flex items-center gap-[6px] text-[17px] leading-[20px] tracking-[0.06em]" style={{ color: props.focused || active() ? '#fff' : 'rgb(255 255 255 / 0.7)' }}>
          {props.name}
          {/* Changed from the film as opened: the same dot the rail uses. */}
          <Show when={props.changed}>
            <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
          </Show>
        </span>
        <span class="absolute top-[4px] right-[12px] origin-right font-semibold tabular-nums transition-transform duration-100" style={{ color: '#fff', 'font-size': '22px', 'line-height': '26px', transform: active() ? 'scale(1.2)' : 'none' }}>
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

/** Two to four options side by side, the chosen one a white plate: ON/OFF and
 * other short choices, where a strip would hide what there is to choose. */
export function Switch(props: { items: readonly string[]; value: number; focused: boolean; pick: (i: number) => void; onFocus?: () => void }) {
  return (
    <div class="flex h-[47px] items-center gap-[4px] px-[5px]" style={TRACK(props.focused)} onPointerDown={() => props.onFocus?.()}>
      <For each={[...props.items]}>
        {(item, i) => (
          <button
            type="button"
            class={`flex h-[37px] grow basis-0 items-center justify-center rounded-[10px] font-medium ${SMALL}`}
            style={props.value === i() ? { background: '#fff', color: '#000' } : { color: 'rgb(255 255 255 / 0.6)' }}
            onClick={() => props.pick(i())}
          >
            {item}
          </button>
        )}
      </For>
    </div>
  );
}
