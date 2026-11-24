import { createEffect, onSettled } from 'solid-js';
import { CustomSettingsApp } from '../../docs/design/custom-settings-editor/app';
import { designLook } from '../../design/looks';
import { CameraKeys, Fit, type CameraKey } from '../../design/screen';
import './film-editor-demo.css';

declare const __FUJIMOD_FILM_EDITOR_DEMO__: boolean;

const SCREEN_WIDTH = 720;

/** The camera demo's name for each camera key. */
const CAMERA_KEYS: Record<CameraKey, string> = {
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  ok: 'Enter',
  back: 'Escape',
  'dial-': '[',
  'dial+': ']',
  q: 'q',
  fn: 'fn',
  play: 'play',
};

/** The computer keyboard's keys for the camera's. */
const KEYBOARD: Record<string, CameraKey> = {
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  Enter: 'ok',
  ' ': 'ok',
  Escape: 'back',
  Backspace: 'back',
  '[': 'dial-',
  ']': 'dial+',
  q: 'q',
  f: 'fn',
  p: 'play',
};

/** The prototype and the on-camera editor side by side at one size. Every
 * key, on-screen button and touch goes to both, at the same screen point. */
export function FilmEditorCompare() {
  let frame: HTMLIFrameElement | undefined;
  let prototype: HTMLDivElement | undefined;
  let sendPrototype: ((key: CameraKey) => void) | undefined;

  const send = (key: CameraKey) => {
    sendPrototype?.(key);
    frame?.contentWindow?.postMessage({ type: 'focal-preview-key', key: CAMERA_KEYS[key] }, window.location.origin);
  };

  /* The camera's material indices, and its glass dials: LIGHTNESS 50 is the
     prototype's 0, the darkest glass. */
  const LOOK_INDEX = { glass: 0, contrast: 1, fujimod: 2 } as const;
  const sendLook = () => {
    const chosen = designLook();
    frame?.contentWindow?.postMessage(
      {
        type: 'focal-preview-material',
        look: LOOK_INDEX[chosen.look],
        opacity: Math.round(chosen.opacity * 100),
        contrast: Math.round(chosen.contrast * 100),
        lightness: Math.round(50 + chosen.lightness * 50),
        rounded: chosen.rounded,
        uppercase: chosen.uppercase,
        mark: chosen.mark,
      },
      window.location.origin,
    );
  };
  createEffect(designLook, sendLook);

  onSettled(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.metaKey || event.ctrlKey || event.altKey) return;
      const key = KEYBOARD[event.key];
      if (!key) return;
      event.preventDefault();
      send(key);
    };
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  });

  /* A touch at a screen point on either side, in the camera's pixels. */
  let target: Element | null = null;
  let start: [number, number] | null = null;
  /* A vertical drag scrolls the prototype's column, as a finger does natively. */
  let scroller: Element | null = null;
  let vertical = false;
  let last = 0;
  const prototypeEvent = (type: string, x: number, y: number, buttons: number) => {
    const screen = prototype?.querySelector('.fm-screen');
    if (!screen) return;
    const box = screen.getBoundingClientRect();
    const scale = box.width / SCREEN_WIDTH;
    const clientX = box.left + x * scale;
    const clientY = box.top + y * scale;
    if (type === 'pointerdown') {
      target = document.elementsFromPoint(clientX, clientY).find((element) => screen.contains(element)) ?? null;
      scroller = null;
      for (let node = target; node && node !== screen; node = node.parentElement)
        if (node.scrollHeight > node.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(node).overflowY)) {
          scroller = node;
          break;
        }
      vertical = false;
      last = y;
    }
    if (!target) return;
    if (type === 'pointermove' && start && scroller) {
      if (!vertical && Math.abs(y - start[1]) > 8 && Math.abs(y - start[1]) > Math.abs(x - start[0])) vertical = true;
      if (vertical) scroller.scrollTop -= y - last;
      last = y;
    }
    const init = { bubbles: true, cancelable: true, composed: true, clientX, clientY, pointerId: 1, pointerType: 'mouse', isPrimary: true, button: 0, buttons };
    target.dispatchEvent(new PointerEvent(type, init));
    if (type === 'pointerdown') target.dispatchEvent(new MouseEvent('mousedown', init));
    if (type === 'pointerup') {
      target.dispatchEvent(new MouseEvent('mouseup', init));
      if (start && Math.hypot(x - start[0], y - start[1]) < 8) target.dispatchEvent(new MouseEvent('click', init));
      target = null;
    }
  };
  const touch = (phase: 1 | 2 | 3, event: PointerEvent) => {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const scale = box.width / SCREEN_WIDTH;
    const x = (event.clientX - box.left) / scale;
    const y = (event.clientY - box.top) / scale;
    if (phase === 1) start = [x, y];
    frame?.contentWindow?.postMessage({ type: 'focal-preview-touch', phase, x, y }, window.location.origin);
    prototypeEvent(phase === 1 ? 'pointerdown' : phase === 2 ? 'pointermove' : 'pointerup', x, y, phase === 3 ? 0 : 1);
    if (phase === 3) start = null;
  };
  const surface = () => (
    <div
      class="film-editor-compare-touch"
      onPointerDown={(event) => {
        touch(1, event);
        /* After the prototype's handlers, which capture the same pointer. */
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (start) touch(2, event);
      }}
      onPointerUp={(event) => {
        if (start) touch(3, event);
      }}
      onPointerCancel={(event) => {
        if (start) touch(3, event);
      }}
      onWheel={(event) => {
        const screen = prototype?.querySelector('.fm-screen');
        const box = event.currentTarget.getBoundingClientRect();
        const scale = box.width / SCREEN_WIDTH;
        const own = screen?.getBoundingClientRect();
        if (!screen || !own) return;
        const point = document.elementsFromPoint(own.left + (event.clientX - box.left) / scale * (own.width / SCREEN_WIDTH), own.top + (event.clientY - box.top) / scale * (own.width / SCREEN_WIDTH));
        const scroller = point.find((element) => screen.contains(element) && element.scrollHeight > element.clientHeight + 1 && getComputedStyle(element).overflowY !== 'visible');
        scroller?.scrollBy({ top: event.deltaY });
      }}
    />
  );

  return (
    <div class="film-editor-compare">
      <div class="film-editor-compare-screens">
        <figure>
          <figcaption>HTML prototype</figcaption>
          <div class="film-editor-compare-screen" ref={(element) => (prototype = element)}>
            <CustomSettingsApp keys={false} connect={(input) => (sendPrototype = input)} />
            {surface()}
          </div>
        </figure>
        {__FUJIMOD_FILM_EDITOR_DEMO__ && (
          <figure>
            <figcaption>FocalUI WebAssembly</figcaption>
            <div class="film-editor-compare-screen">
              <Fit>
                <iframe ref={(element) => (frame = element)} src="/demo/film-editor/index.html" title="Film Editor" width={720} height={480} onLoad={sendLook} />
              </Fit>
              {surface()}
            </div>
          </figure>
        )}
      </div>
      <CameraKeys send={send} />
    </div>
  );
}
