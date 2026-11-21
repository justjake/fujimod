import { For, Show, createEffect, createMemo, createSignal } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { ACCENT, CHOSEN, CHOSEN_INK, HEADING, SMALL, TEXT, TRACK, WELL } from './materials';

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
  /** Each cell's center and width along the strip, measured once laid out. */
  const [cells, setCells] = createSignal<{ center: number; width: number }[]>([]);
  createEffect(
    () => [props.width, props.items] as const,
    () => {
      requestAnimationFrame(() => {
        const list = track ? (Array.from(track.children) as HTMLElement[]) : [];
        setCells(list.map((c) => ({ center: c.offsetLeft + c.offsetWidth / 2, width: c.offsetWidth })));
      });
    },
  );
  const spacing = () => {
    const c = cells();
    return c.length > 1 ? (c[c.length - 1].center - c[0].center) / (c.length - 1) : 60;
  };
  /** A cell's center or width at a fractional position, extended past the ends. */
  const at = (p: number, key: 'center' | 'width') => {
    const c = cells();
    if (c.length === 0) return key === 'width' ? 56 : 0;
    const last = c.length - 1;
    if (p <= 0) return key === 'center' ? c[0].center + p * spacing() : c[0].width;
    if (p >= last) return key === 'center' ? c[last].center + (p - last) * spacing() : c[last].width;
    const i = Math.floor(p);
    return c[i][key] + (c[i + 1][key] - c[i][key]) * (p - i);
  };
  const dial = useDialGesture({
    count: () => props.items.length,
    index: () => props.value,
    spacing,
    commit: (i) => props.pick(i),
    onFocus: () => props.onFocus?.(),
    /* A tap chooses the cell under the finger. */
    tap: (x, box) => {
      const local = (x - box.left) * (props.width / box.width) + offset();
      const c = cells();
      if (c.length === 0) return;
      let best = 0;
      c.forEach((cell, i) => {
        if (Math.abs(cell.center - local) < Math.abs(c[best].center - local)) best = i;
      });
      props.pick(best);
    },
  });
  /* The chosen cell always sits under the fixed plate in the middle. */
  const offset = createMemo(() => at(dial.shown(), 'center') - props.width / 2);
  const plate = createMemo(() => at(dial.shown(), 'width'));
  const ends = (): [boolean, boolean] => {
    const c = cells();
    const o = offset();
    return [c.length > 0 && c[0].center - c[0].width / 2 < o, c.length > 0 && c[c.length - 1].center + c[c.length - 1].width / 2 > o + props.width];
  };
  const motion = () => (dial.active() ? 'none' : 'transform 150ms ease-out, width 150ms ease-out');
  return (
    <div
      class="relative shrink-0 overflow-hidden select-none"
      style={{ width: `${props.width}px`, height: `${props.height ?? 88}px`, 'touch-action': 'pan-y', ...TRACK(props.focused || dial.active()) }}
      {...dial.handlers}
    >
      <Show when={props.name !== undefined}>
        <span class={`absolute top-[9px] left-[12px] flex items-center gap-[6px] ${TEXT} tracking-[0.06em]`} style={{ color: props.focused ? '#fff' : 'rgb(255 255 255 / 0.7)' }}>
          {props.name}
          <Show when={props.changed}>
            <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
          </Show>
        </span>
      </Show>
      <div class="absolute inset-x-0 bottom-0 overflow-hidden" style={{ top: props.name === undefined ? '0' : 'auto', height: props.name === undefined ? 'auto' : '44px', ...WELL, 'border-radius': props.name === undefined ? 'inherit' : '0 0 13px 13px' }}>
        {/* The fixed plate: it stays in the middle and takes the chosen cell's width. */}
        <span
          class="absolute inset-y-[5px] left-1/2 -translate-x-1/2 rounded-[9px]"
          style={{ width: `${plate()}px`, ...CHOSEN, transition: dial.active() ? 'none' : 'width 150ms ease-out' }}
        />
        <div class="absolute inset-0" style={{ 'mask-image': 'linear-gradient(to right, transparent, #000 32px, #000 calc(100% - 32px), transparent)' }}>
        <div
          ref={(element) => (track = element)}
          class="absolute inset-y-0 left-0 flex items-stretch gap-[2px] py-[5px]"
          style={{ transform: `translateX(${-offset()}px)`, transition: motion(), 'will-change': 'transform' }}
        >
          <For each={[...props.items]}>
            {(item, i) => {
              const distance = () => Math.abs(i() - dial.shown());
              const ink = () => (distance() < 0.5 ? CHOSEN_INK : 'rgb(255 255 255 / 0.7)');
              return (
                <span
                  class={`relative flex shrink-0 items-center justify-center font-medium whitespace-nowrap ${TEXT} ${typeof item === 'string' ? 'px-[7px]' : 'px-[3px]'}`}
                  style={{ color: ink() }}
                >
                  {typeof item === 'string' ? item : item.art(ink())}
                </span>
              );
            }}
          </For>
        </div>
        </div>
        <For each={[0, 1] as const}>
          {(side) => (
            <Show when={ends()[side]}>
              <span class="pointer-events-none absolute inset-y-0 flex items-center px-[3px] text-white/80" style={{ [side ? 'right' : 'left']: '0' }}>
                <Chevron dir={side ? 'right' : 'left'} size={14} />
              </span>
            </Show>
          )}
        </For>
      </div>
      {/* Focus is drawn last, over the recessed strip, so it is the same
          bright outline as everywhere else. */}
      <Show when={props.focused || dial.active()}>
        <span class="pointer-events-none absolute inset-0 z-20 rounded-[inherit]" style={{ 'box-shadow': 'var(--m-focus-edge)' }} />
      </Show>
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
  /* Without a name every step is labeled, so steps sit as far apart as the
     widest label needs (the noise-reduction mod's words). */
  const widest = Math.max(...Array.from({ length: count() }, (_, i) => (props.format ?? signed)(props.min + i * props.step).length));
  const SPACING = props.name === undefined ? Math.max(38, widest * 13 + 10) : count() <= 11 ? 30 : count() <= 21 ? 20 : 15;
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
  const chevronInk = (atEnd: boolean) => (atEnd ? 'rgb(255 255 255 / 0.2)' : props.focused || active() ? '#fff' : 'rgb(255 255 255 / 0.45)');
  return (
    <div
      class="relative shrink-0 overflow-hidden select-none"
      style={{ width: `${props.width}px`, height: `${props.height ?? 88}px`, 'touch-action': 'pan-y', ...TRACK(props.focused || active()) }}
      {...dial.handlers}
    >
      <Show when={props.name !== undefined}>
        <span class={`absolute top-[9px] left-[12px] flex items-center gap-[6px] ${TEXT} tracking-[0.06em]`} style={{ color: props.focused || active() ? '#fff' : 'rgb(255 255 255 / 0.7)' }}>
          {props.name}
          {/* Changed from the film as opened: the same dot the rail uses. */}
          <Show when={props.changed}>
            <span class="size-[8px] rounded-full" style={{ background: ACCENT }} />
          </Show>
        </span>
        {/* The value between chevrons: LEFT and RIGHT step it. A chevron dims
            where the scale stops; both are faint until the row is focused. */}
        <span class="absolute top-[6px] right-[6px] flex items-center gap-[2px]">
          <span style={{ color: chevronInk(props.value <= props.min) }}>
            <Chevron dir="left" size={18} />
          </span>
          <span class={`origin-center font-semibold tabular-nums transition-transform duration-100 ${HEADING}`} style={{ color: '#fff', transform: active() ? 'scale(1.2)' : 'none' }}>
            {label(props.value)}
          </span>
          <span style={{ color: chevronInk(props.value >= props.max) }}>
            <Chevron dir="right" size={18} />
          </span>
        </span>
      </Show>
      {/* The scale: a recessed strip along the bottom, the track's colors on
          its bottom edge, labels over the major ticks, the mark at the center. */}
      <div class="absolute inset-x-0 bottom-0" style={{ top: props.name === undefined ? '0' : 'auto', height: props.name === undefined ? 'auto' : '44px', ...WELL, 'border-radius': props.name === undefined ? 'inherit' : '0 0 13px 13px' }}>
        <div class="absolute inset-0" style={{ 'mask-image': 'linear-gradient(to right, transparent, #000 32px, #000 calc(100% - 32px), transparent)' }}>
          <div class="absolute inset-y-0 left-0" style={{ transform: `translateX(${props.width / 2 - shown() * SPACING}px)`, transition: active() ? 'none' : 'transform 160ms ease-out', 'will-change': 'transform' }}>
            <For each={Array.from({ length: count() }, (_, i) => i)}>
              {(i) => {
                const major = () => (i - Math.round(zero())) % MAJOR === 0;
                const near = () => (active() ? Math.max(0, 1 - Math.abs(i - shown()) / 2.5) : 0);
                return (
                  <>
                    <Show when={major()}>
                      <span class={`absolute top-[3px] -translate-x-1/2 font-medium tabular-nums ${SMALL}`} style={{ left: `${i * SPACING}px`, color: `rgb(255 255 255 / ${0.55 + near() * 0.45})` }}>
                        {label(props.min + i * props.step)}
                      </span>
                    </Show>
                    <span
                      class="fm-round absolute bottom-[6px] w-[3px] -translate-x-1/2 rounded-[1.5px]"
                      style={{ left: `${i * SPACING}px`, height: `${major() ? 20 : 12}px`, background: `rgb(255 255 255 / ${(major() ? 0.75 : 0.45) + near() * 0.25})` }}
                    />
                    <Show when={i < count() - 1 && SPACING >= 15}>
                      <For each={[1, 2]}>
                        {(third) => <span class="fm-round absolute bottom-[6px] h-[7px] w-[2px] -translate-x-1/2 rounded-[1px]" style={{ left: `${(i + third / 3) * SPACING}px`, background: 'rgb(255 255 255 / 0.28)' }} />}
                      </For>
                    </Show>
                  </>
                );
              }}
            </For>
          </div>
        </div>
        <Show when={props.track}>{(track) => <div class="absolute inset-x-0 bottom-0 h-[4px]" style={{ background: track() }} />}</Show>
        {/* Focused, LEFT and RIGHT step it: a chevron at each end, dim where
            the scale stops. */}
        <Show when={props.focused && props.name === undefined}>
          <For each={[0, 1] as const}>
            {(side) => (
              <span
                class="pointer-events-none absolute inset-y-0 flex items-center px-[3px]"
                style={{ [side ? 'right' : 'left']: '0', color: (side ? props.value >= props.max : props.value <= props.min) ? 'rgb(255 255 255 / 0.25)' : '#fff' }}
              >
                <Chevron dir={side ? 'right' : 'left'} size={16} />
              </span>
            )}
          </For>
        </Show>
        <span class="fm-round absolute bottom-[4px] w-[4px] -translate-x-1/2 rounded-[2px] transition-[height] duration-100" style={{ left: `${props.width / 2}px`, height: active() ? '30px' : '24px', background: 'var(--m-mark)', 'box-shadow': '0 0 0 1px rgb(0 0 0 / 0.55)' }} />
      </div>
      {/* Focus is drawn last, over the recessed strip, so it is the same
          bright outline as everywhere else. */}
      <Show when={props.focused || active()}>
        <span class="pointer-events-none absolute inset-0 z-20 rounded-[inherit]" style={{ 'box-shadow': 'var(--m-focus-edge)' }} />
      </Show>
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
            class={`flex h-[37px] grow basis-0 items-center justify-center rounded-[10px] font-medium ${TEXT}`}
            style={props.value === i() ? CHOSEN : { color: 'rgb(255 255 255 / 0.6)' }}
            onClick={() => props.pick(i())}
          >
            {item}
          </button>
        )}
      </For>
    </div>
  );
}
