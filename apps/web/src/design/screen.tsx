import { For, Show, createSignal, onSettled } from 'solid-js';
import type { JSX } from '@solidjs/web';

import { ACCENT, GLASS, SMALL } from './materials';

/** The camera's screen, in its own pixels. */
export const SCREEN = { width: 720, height: 480 } as const;

/** The X70's screen is 3.0 inches across its diagonal at 3:2: 2.50 by 1.66
 * inches, 288 camera pixels to the inch. */
const SCREEN_INCHES = (3 * 3) / Math.sqrt(13);

const CALIBRATION_KEY = 'fujimod-design-css-px-per-inch';

/** CSS pixels per physical inch on this display. Browsers do not report it,
 * so it starts from the usual figure for the kind of display (an iPhone's
 * 460-ppi panel at 3 device pixels per CSS pixel is 153; an iPad's 264 at 2
 * is 132; a Mac laptop's is near 113) and the viewer can calibrate it. */
function estimatePxPerInch(): number {
  if (typeof window === 'undefined') return 113;
  try {
    const saved = Number(window.localStorage.getItem(CALIBRATION_KEY));
    if (saved > 0) return saved;
  } catch {
    /* No storage: use the estimate. */
  }
  const touch = window.matchMedia('(pointer: coarse)').matches;
  if (touch && Math.min(window.screen.width, window.screen.height) < 500) return 153;
  if (touch) return 132;
  return 113;
}

/** Whether prototype screens are drawn at the camera screen's physical size. */
export const [actualSize, setActualSize] = createSignal(false);

const [pxPerInchValue, setPxPerInchValue] = createSignal(113);
/** This display's CSS pixels per inch. */
export const pxPerInch = pxPerInchValue;

/** Calibrates the display's CSS pixels per inch, remembered on this device. */
export function setPxPerInch(value: number) {
  setPxPerInchValue(value);
  try {
    window.localStorage.setItem(CALIBRATION_KEY, String(value));
  } catch {
    /* No storage: the calibration lasts for this visit. */
  }
}

/** Loads this device's calibration; called once a page is in the browser. */
export function loadCalibration() {
  setPxPerInchValue(estimatePxPerInch());
}

/** A camera screen at its own size, scaled down to fit a narrow page; with
 * ACTUAL SIZE on, at the camera screen's physical size. */
export function Fit(props: { children: JSX.Element }) {
  let host: HTMLDivElement | undefined;
  const [fitScale, setFitScale] = createSignal(1);
  onSettled(() => {
    const element = host;
    if (!element) return;
    const observer = new ResizeObserver(() => setFitScale(Math.min(1, element.clientWidth / SCREEN.width)));
    observer.observe(element);
    return () => observer.disconnect();
  });
  const scale = () => (actualSize() ? (SCREEN_INCHES * pxPerInch()) / SCREEN.width : fitScale());
  return (
    <div
      ref={(element) => (host = element)}
      class="overflow-hidden"
      style={
        actualSize()
          ? /* Centered on the page, past its side padding if it must be. */
            { width: `${SCREEN.width * scale()}px`, height: `${SCREEN.height * scale()}px`, 'margin-left': `calc(50% - ${(SCREEN.width * scale()) / 2}px)` }
          : { width: '100%', 'max-width': `${SCREEN.width}px`, height: `${SCREEN.height * scale()}px` }
      }
    >
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
      aria-label={key}
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
      <span class={`flex h-[26px] min-w-[26px] items-center justify-center rounded-[6px] bg-white px-[4px] font-semibold text-black ${SMALL}`}>{props.glyph}</span>
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
            <Lazy>
              <Fit>
                <CameraScreen>{props.render(entry.state)}</CameraScreen>
              </Fit>
            </Lazy>
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

/** Builds its children once they come near the viewport, and keeps them;
 * until then holds a screen's space. */
function Lazy(props: { children: JSX.Element }) {
  const [shown, setShown] = createSignal(false);
  let box: HTMLDivElement | undefined;
  onSettled(() => {
    if (!box) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setShown(true);
          observer.disconnect();
        }
      },
      { rootMargin: '400px 0px' },
    );
    observer.observe(box);
    return () => observer.disconnect();
  });
  return (
    <div ref={(element) => (box = element)} style={{ 'aspect-ratio': '3 / 2' }}>
      <Show when={shown()}>{props.children}</Show>
    </div>
  );
}
