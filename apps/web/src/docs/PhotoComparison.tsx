import { For, Show, createSignal, onCleanup, onSettled } from 'solid-js';
import type { Photo } from '../home/feature-media';
import './photo-comparison.css';

const mediaRoot = '/artifacts/docs/';

/** Device pixels per photograph pixel: hovering shows 1:1, a click magnifies. */
const zooms = [1, 4] as const;

/** A point on every photograph, as fractions of its width and height. */
type Point = { u: number; v: number };

/** Photographs side by side from the sidebar's edge to the window's edge, or
 * across the whole window when the sidebar is above. With the pointer over a
 * photograph, every photograph shows its 1:1 pixels around the same point,
 * which follows the pointer and is marked by the same cross; a click
 * magnifies further and a minimap shows the part in view. On touch screens a
 * tap zooms, another tap magnifies, dragging follows the finger, and a third
 * tap returns. */
export function PhotoComparison(props: { photos: readonly Photo[] }) {
  const [point, setPoint] = createSignal<Point>();
  const [zoom, setZoom] = createSignal(0);
  const [dpr, setDpr] = createSignal(2);
  const [frameWidth, setFrameWidth] = createSignal(1);
  let row!: HTMLDivElement;

  onSettled(() => {
    const fit = () => {
      row.style.marginLeft = '';
      row.style.width = '';
      const natural = row.getBoundingClientRect().left;
      const sidebar = document.querySelector('.docs-sidebar')?.getBoundingClientRect();
      const beside = sidebar && sidebar.right <= natural;
      const left = beside ? sidebar.right + 24 : 0;
      row.style.marginLeft = `${left - natural}px`;
      row.style.width = `${document.documentElement.clientWidth - left}px`;
      setDpr(devicePixelRatio);
      setFrameWidth(row.querySelector('.photo-frame')?.clientWidth ?? 1);
    };
    const observer = new ResizeObserver(fit);
    observer.observe(document.documentElement);
    fit();
    return () => observer.disconnect();
  });

  /** CSS pixels per photograph pixel. */
  const scale = () => zooms[zoom()] / dpr();

  const pointIn = (event: PointerEvent): Point | undefined => {
    const frame = (event.target as Element).closest('.photo-frame');
    if (!frame) return undefined;
    const box = frame.getBoundingClientRect();
    return {
      u: Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)),
      v: Math.min(1, Math.max(0, (event.clientY - box.top) / box.height)),
    };
  };

  const close = () => {
    setPoint(undefined);
    setZoom(0);
  };

  // Touch: taps step through 1:1, magnified and back; dragging follows.
  let touch: { x: number; y: number; dragged: boolean } | undefined;
  const down = (event: PointerEvent) => {
    if (event.pointerType === 'mouse' || !pointIn(event)) return;
    touch = { x: event.clientX, y: event.clientY, dragged: false };
    if (point()) row.setPointerCapture(event.pointerId);
  };
  const move = (event: PointerEvent) => {
    if (event.pointerType === 'mouse') {
      const at = pointIn(event);
      if (at) setPoint(at);
      else close();
      return;
    }
    if (!touch || !point()) return;
    if (Math.hypot(event.clientX - touch.x, event.clientY - touch.y) > 4) touch.dragged = true;
    if (touch.dragged) setPoint(pointIn(event) ?? point());
  };
  const up = (event: PointerEvent) => {
    if (event.pointerType === 'mouse') {
      if (point()) setZoom((zoom() + 1) % zooms.length);
      return;
    }
    const tapped = touch && !touch.dragged;
    touch = undefined;
    if (!tapped) return;
    if (!point()) setPoint(pointIn(event));
    else if (zoom() + 1 < zooms.length) setZoom(zoom() + 1);
    else close();
  };
  const leave = (event: PointerEvent) => {
    if (event.pointerType === 'mouse') close();
  };
  const outside = (event: PointerEvent) => {
    if (point() && !row.contains(event.target as Node)) close();
  };
  window.addEventListener('pointerdown', outside);
  onCleanup(() => window.removeEventListener('pointerdown', outside));

  return (
    <div
      ref={(element) => (row = element)}
      class={{ 'photo-comparison': true, zoomed: !!point() }}
      style={{ '--count': props.photos.length, '--hair': `${1 / dpr()}px` }}
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
                {(at) => {
                  /** The fraction of the photograph's width (and height) in view. */
                  const seen = () => Math.min(1, frameWidth() / (photo.width * scale()));
                  return (
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
                      <svg
                        class="photo-cross"
                        viewBox="-10 -10 20 20"
                        style={{ left: `${at().u * 100}%`, top: `${at().v * 100}%` }}
                      >
                        <path d="M-10 0H-3M3 0H10M0 -10V-3M0 3V10" class="photo-cross-halo" />
                        <path d="M-10 0H-3M3 0H10M0 -10V-3M0 3V10" class="photo-cross-line" />
                      </svg>
                      <div
                        class="photo-minimap"
                        style={{ 'aspect-ratio': `${photo.width} / ${photo.height}` }}
                      >
                        <span
                          style={{
                            left: `${at().u * (1 - seen()) * 100}%`,
                            top: `${at().v * (1 - seen()) * 100}%`,
                            width: `${seen() * 100}%`,
                            height: `${seen() * 100}%`,
                          }}
                        />
                      </div>
                    </>
                  );
                }}
              </Show>
            </div>
            <figcaption>{photo.label}</figcaption>
          </figure>
        )}
      </For>
    </div>
  );
}
