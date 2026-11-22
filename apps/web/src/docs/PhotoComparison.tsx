import { For, Show, createSignal, onCleanup, onSettled } from 'solid-js';
import type { Photo } from '../home/feature-media';
import './photo-comparison.css';

const mediaRoot = '/artifacts/docs/';

/** A point on every photograph, as fractions of its width and height. */
type Point = { u: number; v: number };

/** Photographs side by side from the text column to the window's edge. With
 * the pointer over a photograph, every photograph shows its 1:1 pixels around
 * the same point, which follows the pointer and is marked by the same cross.
 * On touch screens a tap zooms, dragging follows the finger, and a tap without
 * dragging returns. */
export function PhotoComparison(props: { photos: readonly Photo[] }) {
  const [point, setPoint] = createSignal<Point>();
  const [scale, setScale] = createSignal(1);
  let row!: HTMLDivElement;

  onSettled(() => {
    const fit = () => {
      row.style.width = '';
      const width = document.documentElement.clientWidth - row.getBoundingClientRect().left;
      row.style.width = `${width}px`;
      row.style.setProperty('--dpr', String(devicePixelRatio));
      setScale(1 / devicePixelRatio);
    };
    const observer = new ResizeObserver(fit);
    observer.observe(document.documentElement);
    fit();
    return () => observer.disconnect();
  });

  const pointIn = (event: PointerEvent): Point | undefined => {
    const frame = (event.target as Element).closest('.photo-frame');
    if (!frame) return undefined;
    const box = frame.getBoundingClientRect();
    return {
      u: Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)),
      v: Math.min(1, Math.max(0, (event.clientY - box.top) / box.height)),
    };
  };

  // Touch: a tap zooms; while zoomed, dragging follows and a tap returns.
  let touch: { x: number; y: number; dragged: boolean } | undefined;
  const down = (event: PointerEvent) => {
    if (event.pointerType === 'mouse' || !pointIn(event)) return;
    touch = { x: event.clientX, y: event.clientY, dragged: false };
    if (point()) row.setPointerCapture(event.pointerId);
  };
  const move = (event: PointerEvent) => {
    if (event.pointerType === 'mouse') return void setPoint(pointIn(event));
    if (!touch || !point()) return;
    if (Math.hypot(event.clientX - touch.x, event.clientY - touch.y) > 4) touch.dragged = true;
    if (touch.dragged) setPoint(pointIn(event) ?? point());
  };
  const up = (event: PointerEvent) => {
    const tapped = touch && !touch.dragged;
    touch = undefined;
    if (event.pointerType === 'mouse' || !tapped) return;
    setPoint(point() ? undefined : pointIn(event));
  };
  const leave = (event: PointerEvent) => {
    if (event.pointerType === 'mouse') setPoint(undefined);
  };
  const outside = (event: PointerEvent) => {
    if (point() && !row.contains(event.target as Node)) setPoint(undefined);
  };
  window.addEventListener('pointerdown', outside);
  onCleanup(() => window.removeEventListener('pointerdown', outside));

  return (
    <div
      ref={(element) => (row = element)}
      class={{ 'photo-comparison': true, zoomed: !!point() }}
      style={{ '--count': props.photos.length }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerLeave={leave}
      onPointerCancel={() => (touch = undefined)}
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
                  <>
                    <img
                      class="photo-pixels"
                      src={mediaRoot + photo.full}
                      alt=""
                      draggable={false}
                      style={{
                        left: `${at().u * 100}%`,
                        top: `${at().v * 100}%`,
                        width: `${photo.width * scale()}px`,
                        height: `${photo.height * scale()}px`,
                        transform: `translate(${-at().u * photo.width * scale()}px, ${-at().v * photo.height * scale()}px)`,
                      }}
                    />
                    <span
                      class="photo-cross"
                      style={{ left: `${at().u * 100}%`, top: `${at().v * 100}%` }}
                    />
                  </>
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
