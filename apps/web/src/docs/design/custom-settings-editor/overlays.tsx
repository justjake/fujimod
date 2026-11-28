import PHOTO from '../../../engine/assets/default-scene.png';
import { For } from 'solid-js';

import { Confirmation } from '../../../design/confirmation';
import { Chevron } from '../../../design/controls';
import { DETAIL } from '../../../design/materials';
import {
  PICTURE_LEFT,
  PICTURE_WIDTH,
  RAWS,
  activeFilmEntry,
  applyReflectModalChoice,
  applySwitchModalChoice,
  keyCaps,
  switchModalOptions,
  typeKey,
  type KeyCap,
  type Typing,
  type UnifiedState,
} from './model';

/* ---- WB SHIFT 19x19 2D grid ------------------------------------------------------ */

export const SHIFT_WIDTH = 236;
export const SHIFT_HEIGHT = 236;
export const SHIFT_CAPTION = 30;

export function ShiftGrid(props: {
  r: number;
  b: number;
  onShift: (r: number, b: number) => void;
}) {
  const across = SHIFT_WIDTH / 19;
  const down = SHIFT_HEIGHT / 19;
  const x = (r: number) => (r + 9) * across + across / 2;
  const y = (b: number) => (9 - b) * down + down / 2;
  const mix = (a: number[], b: number[], t: number) =>
    a.map((c, i) => Math.round(c + (b[i] - c) * t));
  const dot = (r: number, b: number) => {
    const side = mix([255, 255, 255], r < 0 ? [40, 220, 40] : [235, 40, 70], Math.abs(r) / 9);
    const [red, green, blue] = mix(side, b > 0 ? [40, 110, 255] : [240, 215, 0], Math.abs(b) / 12);
    return `rgb(${red} ${green} ${blue})`;
  };
  const steps = Array.from({ length: 19 }, (_, i) => i - 9);
  return (
    <svg
      width={SHIFT_WIDTH}
      height={SHIFT_HEIGHT}
      class="block"
      onClick={(e) => {
        const box = e.currentTarget.getBoundingClientRect();
        const r = Math.floor(((e.clientX - box.left) / box.width) * 19) - 9;
        const b = 9 - Math.floor(((e.clientY - box.top) / box.height) * 19);
        props.onShift(Math.max(-9, Math.min(9, r)), Math.max(-9, Math.min(9, b)));
      }}
    >
      <For each={steps}>
        {(b) => (
          <For each={steps}>
            {(r) => <rect x={x(r) - 2} y={y(b) - 2} width={4} height={4} fill={dot(r, b)} />}
          </For>
        )}
      </For>
      <line
        x1={SHIFT_WIDTH / 2}
        x2={SHIFT_WIDTH / 2}
        y1={0}
        y2={SHIFT_HEIGHT}
        stroke="rgb(255 255 255 / 0.85)"
        stroke-width={2}
      />
      <line
        y1={SHIFT_HEIGHT / 2}
        y2={SHIFT_HEIGHT / 2}
        x1={0}
        x2={SHIFT_WIDTH}
        stroke="rgb(255 255 255 / 0.85)"
        stroke-width={2}
      />
      <rect
        x={x(props.r) - 8}
        y={y(props.b) - 8}
        width={16}
        height={16}
        fill="none"
        stroke="#fff"
        stroke-width={2}
      />
    </svg>
  );
}

/* ---- RAW frame strip over top of right picture ----------------------------------- */

export const RAW_STRIP_BAND = 96;

export function RawStrip(props: { s: UnifiedState; update: (s: UnifiedState) => void }) {
  const s = () => props.s;
  const focused = () => {
    const f = s().focus;
    return f.kind === 'raw' && f.item === 'strip';
  };
  const w = 104;
  const h = 68;
  const gap = 8;
  const offset = () => {
    const end = (s().crop - 1) * (w + gap) + w;
    return 8 - Math.max(0, end - (PICTURE_WIDTH - 16));
  };
  return (
    <div
      class="absolute top-0 overflow-hidden"
      style={{
        left: `${PICTURE_LEFT}px`,
        width: `${PICTURE_WIDTH}px`,
        height: `${RAW_STRIP_BAND}px`,
        background: 'linear-gradient(rgb(0 0 0 / 0.72), rgb(0 0 0 / 0))',
      }}
    >
      <div
        class="absolute top-[8px] flex"
        style={{
          gap: `${gap}px`,
          transform: `translateX(${offset()}px)`,
          transition: 'transform 240ms cubic-bezier(0.22, 1, 0.36, 1)',
        }}
      >
        <For each={RAWS}>
          {(raw, i) => {
            const chosen = () => s().crop === i() + 1;
            return (
              <button
                type="button"
                class="flex flex-col items-center gap-[3px]"
                onClick={() =>
                  props.update({
                    ...s(),
                    crop: i() + 1,
                    created: 0,
                    focus: { kind: 'raw', item: 'strip' },
                  })
                }
              >
                <span
                  class="block shrink-0 rounded-[3px] bg-cover"
                  style={{
                    width: `${w}px`,
                    height: `${h}px`,
                    'background-image': `url(${PHOTO})`,
                    'background-size': `${raw.zoom * 100}% auto`,
                    'background-position': `${raw.x * 100}% ${raw.y * 100}%`,
                    opacity: chosen() ? 1 : 0.55,
                    'box-shadow': chosen()
                      ? `0 0 0 2px #000, 0 0 0 ${focused() ? 4 : 3}px ${focused() ? '#fff' : 'rgb(255 255 255 / 0.65)'}`
                      : undefined,
                  }}
                />
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}

/* ---- Modal prompt on film switch when edited ------------------------------------- */

export function FilmSwitchModal(props: { s: UnifiedState; update: (s: UnifiedState) => void }) {
  const modal = () => props.s.switchModal!;
  const options = () => switchModalOptions(props.s);
  const active = () => activeFilmEntry(props.s);
  const target = () => props.s.films.find((f) => f.id === modal().targetFilmId);
  return (
    <Confirmation
      title="SAVE FILM CHANGES?"
      message={`Switching to ${target()?.name ?? 'another film'} will replace edits to ${active().name}.`}
      actions={options().map((opt) => ({
        label: opt.label,
        destructive: opt.id === 'discard',
      }))}
      focused={modal().focus}
      onSelect={(i) => {
        const opt = options()[i];
        if (opt) props.update(applySwitchModalChoice(props.s, opt.id));
      }}
      onCancel={() => props.update({ ...props.s, switchModal: null })}
    />
  );
}

export function ReflectConfirmModal(props: { s: UnifiedState; update: (s: UnifiedState) => void }) {
  const modal = () => props.s.reflectModal!;
  return (
    <Confirmation
      title="REFLECT SHOOTING COND.?"
      message="Current recipe and RAW adjustments will be replaced by this photo's shooting settings."
      actions={[{ label: 'REPLACE SETTINGS', destructive: true }, { label: 'CANCEL' }]}
      focused={modal().focus}
      onSelect={(i) => props.update(applyReflectModalChoice(props.s, i === 0))}
      onCancel={() => props.update({ ...props.s, reflectModal: null })}
    />
  );
}

/* ---- Focal UI Text Keyboard ------------------------------------------------------ */

function DialGlyph() {
  return (
    <svg
      width="26"
      height="22"
      viewBox="0 0 26 22"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
    >
      <ellipse cx="13" cy="7" rx="9" ry="4" />
      <path d="M4 7v5c0 2.2 4 4 9 4s9-1.8 9-4V7M8 19l-2.5 1.5M18 19l2.5 1.5" />
    </svg>
  );
}

function KeyLabel(props: { cap: KeyCap; t: Typing }) {
  const c = props.cap;
  switch (c.kind) {
    case 'caret-left':
      return (
        <span class="flex items-center gap-[8px]">
          <Chevron dir="left" size={18} />
          <DialGlyph />
        </span>
      );
    case 'caret-right':
      return (
        <span class="flex items-center gap-[8px]">
          <DialGlyph />
          <Chevron dir="right" size={18} />
        </span>
      );
    case 'shift':
      return (
        <svg
          width="26"
          height="26"
          viewBox="0 0 24 24"
          fill={props.t.shift === 'off' ? 'none' : 'currentColor'}
          stroke="currentColor"
          stroke-width="2"
          stroke-linejoin="round"
        >
          <path d="M12 3 3 12h5v8h8v-8h5z" />
          {props.t.shift === 'locked' ? <path d="M8 23h8" /> : null}
        </svg>
      );
    case 'delete':
      return (
        <svg
          width="30"
          height="24"
          viewBox="0 0 30 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.4"
          stroke-linejoin="round"
        >
          <path d="M9 3h18v18H9L1 12z" />
          <path d="m14 8 8 8m0-8-8 8" />
        </svg>
      );
  }
  return <>{c.label}</>;
}

export function Keyboard(props: { s: UnifiedState; t: Typing; update: (s: UnifiedState) => void }) {
  const t = () => props.t;
  const title = () => {
    switch (t().target) {
      case 'recipe-name':
        return 'RENAME RECIPE';
      case 'recipe-saveas':
        return 'SAVE NEW RECIPE';
      case 'film-saveas':
        return 'SAVE NEW FILM SIMULATION';
    }
  };
  const fieldLabel = () => {
    switch (t().target) {
      case 'recipe-name':
        return 'RECIPE NAME';
      case 'recipe-saveas':
        return 'NEW RECIPE NAME';
      case 'film-saveas':
        return 'NEW FILM SIMULATION NAME';
    }
  };
  const ROW_TOP = [6, 54, 124, 194, 264];
  const ROW_H = [38, 60, 60, 60, 60];
  const caps = () => keyCaps(t());
  return (
    <div class="absolute inset-0 z-50 flex flex-col" style={{ 'text-transform': 'none' }}>
      <div
        class="flex h-[48px] items-center gap-[14px] px-[18px]"
        style={{ background: 'var(--m-title)' }}
      >
        <button
          type="button"
          class="flex size-[32px] items-center justify-center text-white/70"
          onClick={() => props.update({ ...props.s, keyboard: null })}
        >
          <Chevron dir="left" size={22} />
        </button>
        <span class={`font-semibold tracking-[0.1em] text-white ${DETAIL}`}>{title()}</span>
      </div>
      <div
        class="flex grow flex-col justify-center gap-[6px] px-[20px]"
        style={{ background: 'var(--m-text-area)' }}
      >
        <span class="text-[14px] leading-none font-semibold tracking-[0.1em] text-white/55">
          {fieldLabel()}
        </span>
        <span class="text-[29px] leading-[36px] whitespace-pre text-white">
          {t().text.slice(0, t().cursor)}
          <span class="inline-block h-[28px] w-[3px] translate-y-[4px] bg-[#0a6bdc]" />
          {t().text.slice(t().cursor)}
        </span>
      </div>
      <div class="relative h-[330px] shrink-0" style={{ background: 'var(--m-tray)' }}>
        <For each={caps()}>
          {(cap, i) => {
            const special = cap.kind !== 'char' && cap.kind !== 'space';
            const focused = () => t().focus === i();
            return (
              <button
                type="button"
                class="absolute flex items-center justify-center rounded-[7px] text-white"
                style={{
                  left: `${6 + cap.left * 7.08 + 3}px`,
                  width: `${cap.width * 7.08 - 6}px`,
                  top: `${ROW_TOP[cap.row]}px`,
                  height: `${ROW_H[cap.row]}px`,
                  background:
                    cap.kind === 'return'
                      ? 'var(--m-confirm)'
                      : special
                        ? 'var(--m-key-special)'
                        : 'var(--m-key)',
                  'box-shadow': focused() ? 'var(--m-key-focus)' : 'var(--m-key-edge)',
                  'font-size': cap.kind === 'char' ? '29px' : '21px',
                  'letter-spacing': cap.kind === 'layer' ? '0.12em' : undefined,
                }}
                onClick={() =>
                  props.update(
                    typeKey(
                      { ...props.s, keyboard: { ...t(), focus: i() } },
                      { ...t(), focus: i() },
                      cap,
                    ),
                  )
                }
              >
                <KeyLabel cap={cap} t={t()} />
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}
