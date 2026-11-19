import { For, Show, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { ACCENT, GLASS, SMALL } from './materials';

/** The camera's screen, in its own pixels. */
export const SCREEN = { width: 720, height: 480 } as const;

/** A camera screen at its own size, scaled down to fit a narrow page. */
export function Fit(props: { children: JSX.Element }) {
  let host: HTMLDivElement | undefined;
  const [scale, setScale] = createSignal(1);
  onSettled(() => {
    const element = host;
    if (!element) return;
    const observer = new ResizeObserver(() => setScale(Math.min(1, element.clientWidth / SCREEN.width)));
    observer.observe(element);
    return () => observer.disconnect();
  });
  return (
    <div ref={(element) => (host = element)} class="w-full max-w-[720px] overflow-hidden" style={{ height: `${SCREEN.height * scale()}px` }}>
      <div class="origin-top-left" style={{ transform: `scale(${scale()})`, width: `${SCREEN.width}px` }}>
        {props.children}
      </div>
    </div>
  );
}

/** The camera's keys a prototype answers. */
export type CameraKey = 'left' | 'right' | 'up' | 'down' | 'ok' | 'back' | 'dial-' | 'dial+' | 'q' | 'fn' | 'play';

/** The camera's keys on screen, under a prototype, for touch. */
export function CameraKeys(props: { send: (key: CameraKey) => void }) {
  const button = (key: CameraKey, label: string) => (
    <button
      type="button"
      class="flex h-[52px] min-w-[52px] items-center justify-center rounded-[14px] bg-[#2c2c2e] px-[10px] text-[17px] font-semibold text-white active:bg-[#48484a]"
      onClick={() => props.send(key)}
    >
      {label}
    </button>
  );
  return (
    <div class="mt-[12px] flex max-w-[720px] flex-wrap items-center justify-between gap-[14px]">
      <div class="grid grid-cols-3 gap-[6px]">
        <span />
        {button('up', '▲')}
        <span />
        {button('left', '◀')}
        {button('ok', 'OK')}
        {button('right', '▶')}
        <span />
        {button('down', '▼')}
        <span />
      </div>
      <div class="grid grid-cols-2 gap-[6px]">
        {button('dial-', 'DIAL −')}
        {button('dial+', 'DIAL +')}
        {button('q', 'Q')}
        {button('fn', 'Fn')}
        {button('play', 'PLAY ▶')}
        {button('back', 'BACK')}
      </div>
    </div>
  );
}

/** A key hint over the picture: the key's glyph on a white plate, and what it does. */
export function KeyHint(props: { glyph: string; label?: string; tap?: () => void; accent?: boolean }) {
  return (
    <button
      type="button"
      class="flex h-[36px] items-center gap-[8px] rounded-[12px] px-[7px] whitespace-nowrap"
      style={{ ...GLASS, ...(props.accent ? { background: ACCENT } : {}) }}
      onClick={(e) => {
        e.stopPropagation();
        props.tap?.();
      }}
    >
      <span class="flex h-[24px] min-w-[24px] items-center justify-center rounded-[6px] bg-white px-[4px] text-[18px] leading-none font-semibold text-black">{props.glyph}</span>
      <Show when={props.label}>
        <span class={`pr-[3px] font-light tracking-[0.04em] ${SMALL}`}>{props.label}</span>
      </Show>
    </button>
  );
}

/** A glass card over the picture: names, values and notes, and fly-out bodies. */
export function Card(props: { class?: string; style?: JSX.CSSProperties; children: JSX.Element }) {
  return (
    <div class={`rounded-[12px] font-semibold ${props.class ?? ''}`} style={{ ...GLASS, ...props.style }}>
      {props.children}
    </div>
  );
}

/** A prototype's review states: each under its title, linkable by its id. */
export function States<T>(props: { title: string; states: { id: string; title: string; state: T }[]; render: (state: T) => JSX.Element }) {
  return (
    <>
      <h2 class="mt-[40px] text-[22px] font-semibold">{props.title}</h2>
      <For each={props.states}>
        {(entry) => (
          <section id={entry.id} class="mt-[28px]">
            <h3 class="mb-[10px] text-[17px] font-semibold">{entry.title}</h3>
            <Fit>
              <CameraScreen>{props.render(entry.state)}</CameraScreen>
            </Fit>
          </section>
        )}
      </For>
    </>
  );
}

/** The camera's 720 by 480 screen. The chosen look's materials apply inside it. */
export function CameraScreen(props: { style?: JSX.CSSProperties; children: JSX.Element }) {
  return (
    <div class="fm-screen relative h-[480px] w-[720px] overflow-hidden bg-black font-sans text-white select-none" style={props.style}>
      {props.children}
    </div>
  );
}
