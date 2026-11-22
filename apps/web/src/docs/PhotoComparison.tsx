import { For, Show, createSignal, onCleanup, onSettled } from 'solid-js';
import type { Photo } from '../home/feature-media';
import './photo-comparison.css';

const mediaRoot = '/artifacts/docs/';
/** Pointer travel, in CSS pixels, that turns a press into a drag. */
const DRAG = 4;

/** A point on every photograph, as fractions of its width and height. */
type Point = { u: number; v: number };

/** Photographs side by side across the window. Hovering shows a 1:1 loupe at
 * the same point of every photograph; clicking or tapping fills each one with
 * its 1:1 pixels, and dragging pans them together. */
export function PhotoComparison(props: { photos: readonly Photo[] }) {
  const [point, setPoint] = createSignal<Point>();
  const [zoomed, setZoomed] = createSignal(false);
  const [scale, setScale] = createSignal(1);
  let row!: HTMLDivElement;

  // The row spans the window, past the text column and the sidebar.
  onSettled(() => {
    const bleed = () => {
      row.style.marginLeft = '0px';
      row.style.marginLeft = `${-row.getBoundingClientRect().left}px`;
      row.style.width = `${document.documentElement.clientWidth}px`;
      setScale(1 / devicePixelRatio);
    };
    const observer = new ResizeObserver(bleed);
    observer.observe(document.documentElement);
    bleed();
    return () => observer.disconnect();
  });

  const pointIn = (event: PointerEvent): Point | undefined => {
    const image = (event.target as Element).closest('.photo-frame');
    if (!image) return undefined;
    const box = image.getBoundingClientRect();
    return {
      u: Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)),
      v: Math.min(1, Math.max(0, (event.clientY - box.top) / box.height)),
    };
  };

  let press: { x: number; y: number; start: Point; dragged: boolean } | undefined;
  const down = (event: PointerEvent) => {
    const at = pointIn(event);
    if (!at) return;
    press = { x: event.clientX, y: event.clientY, start: point() ?? at, dragged: false };
    if (zoomed()) row.setPointerCapture(event.pointerId);
  };
  const move = (event: PointerEvent) => {
    if (press && zoomed()) {
      const dx = event.clientX - press.x;
      const dy = event.clientY - press.y;
      if (!press.dragged && Math.hypot(dx, dy) < DRAG) return;
      press.dragged = true;
      // Dragging moves the pixels with the pointer at 1:1.
      const { width, height } = props.photos[0];
      const s = scale();
      setPoint({
        u: Math.min(1, Math.max(0, press.start.u - dx / (width * s))),
        v: Math.min(1, Math.max(0, press.start.v - dy / (height * s))),
      });
      return;
    }
    if (event.pointerType === 'mouse' && !zoomed()) setPoint(pointIn(event));
  };
  const up = (event: PointerEvent) => {
    const pressed = press;
    press = undefined;
    if (!pressed || pressed.dragged) return;
    if (zoomed()) {
      setZoomed(false);
      if (event.pointerType !== 'mouse') setPoint(undefined);
      else setPoint(pointIn(event));
    } else {
      setPoint(pointIn(event));
      setZoomed(true);
    }
  };
  const leave = (event: PointerEvent) => {
    if (event.pointerType === 'mouse' && !zoomed()) setPoint(undefined);
  };
  const key = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      setZoomed(false);
      setPoint(undefined);
    }
  };
  const outside = (event: PointerEvent) => {
    if (zoomed() && !row.contains(event.target as Node)) {
      setZoomed(false);
      setPoint(undefined);
    }
  };
  window.addEventListener('keydown', key);
  window.addEventListener('pointerdown', outside);
  onCleanup(() => {
    window.removeEventListener('keydown', key);
    window.removeEventListener('pointerdown', outside);
  });

  return (
    <div
      ref={(element) => (row = element)}
      class={{ 'photo-comparison': true, zoomed: zoomed() }}
      style={{ '--count': props.photos.length }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerLeave={leave}
      onPointerCancel={() => (press = undefined)}
    >
      <For each={props.photos}>
        {(photo) => (
          <figure class="photo-cell">
            <div class="photo-frame" style={{ 'aspect-ratio': `${photo.width} / ${photo.height}` }}>
              <img
                src={mediaRoot + photo.image}
                alt={photo.label}
                loading="lazy"
                draggable={false}
              />
              <Show when={point()}>
                {(at) => (
                  <div
                    class={zoomed() ? 'photo-zoom' : 'photo-loupe'}
                    style={
                      zoomed() ? undefined : { left: `${at().u * 100}%`, top: `${at().v * 100}%` }
                    }
                  >
                    <img
                      class="photo-pixels"
                      src={mediaRoot + photo.full}
                      alt=""
                      draggable={false}
                      style={{
                        width: `${photo.width * scale()}px`,
                        height: `${photo.height * scale()}px`,
                        transform: `translate(${-at().u * photo.width * scale()}px, ${-at().v * photo.height * scale()}px)`,
                      }}
                    />
                  </div>
                )}
              </Show>
            </div>
            <figcaption>{photo.label}</figcaption>
          </figure>
        )}
      </For>
    </div>
  );
}
