import { Show } from 'solid-js';
import type { JSX } from '@solidjs/web';

/* Artwork for prototypes. Stock camera artwork is never published: the
   development server serves it from its local review directory, and a
   published page gets no URL and draws its fallback instead. Our own
   artwork follows the same path until it is part of the site's assets. */

const local = (path: string): string | undefined => (import.meta.env.DEV ? `/review/${path}` : undefined);

/** A stock camera vector by its resource id, on the development server only. */
export const stockArtwork = (id: number) => local(`stock-icons/x70-110/ff${id.toString(16).padStart(6, '0')}.svg`);

/** A mod's setting icon by its slug. */
export const modArtwork = (slug: string) => local(`recipe-editor/mod-icons/${slug}.svg`);

/** A FILM ICONS pack icon by its slug. */
export const filmArtwork = (slug: string) => local(`film-icons/a-${slug}.svg`);

/** Where an artwork's ink sits on its canvas, as fractions of the canvas. */
interface Ink {
  aspect: number;
  width: number;
  height: number;
  centerX: number;
  centerY: number;
}

const inks = new Map<string, Promise<Ink>>();

/** Measures an artwork's ink once per URL. */
function measureInk(url: string): Promise<Ink> {
  let ink = inks.get(url);
  if (ink === undefined) {
    ink = (async () => {
      const image = new Image();
      image.src = url;
      await image.decode();
      const w = image.naturalWidth * 4;
      const h = image.naturalHeight * 4;
      const canvas = document.createElement('canvas');
      canvas.width = w;
      canvas.height = h;
      const context = canvas.getContext('2d');
      if (context === null) throw new Error('no 2d canvas');
      context.drawImage(image, 0, 0, w, h);
      const alpha = context.getImageData(0, 0, w, h).data;
      let x0 = w, y0 = h, x1 = -1, y1 = -1;
      for (let y = 0; y < h; y++)
        for (let x = 0; x < w; x++)
          if (alpha[(y * w + x) * 4 + 3] > 40) {
            x0 = Math.min(x0, x);
            x1 = Math.max(x1, x);
            y0 = Math.min(y0, y);
            y1 = Math.max(y1, y);
          }
      if (x1 < 0) return { aspect: w / h, width: 1, height: 1, centerX: 0.5, centerY: 0.5 };
      return { aspect: w / h, width: (x1 - x0 + 1) / w, height: (y1 - y0 + 1) / h, centerX: (x0 + x1 + 1) / 2 / w, centerY: (y0 + y1 + 1) / 2 / h };
    })();
    inks.set(url, ink);
  }
  return ink;
}

/** Scales an artwork so its ink, not its canvas, fills the element's box. */
async function fitInk(element: HTMLElement, url: string) {
  const ink = await measureInk(url);
  if (!element.isConnected) await new Promise(requestAnimationFrame);
  const boxW = element.clientWidth;
  const boxH = element.clientHeight;
  const canvasW = Math.min(boxW, boxH * ink.aspect);
  const canvasH = canvasW / ink.aspect;
  const scale = Math.min(boxW / (ink.width * canvasW), boxH / (ink.height * canvasH));
  element.style.transform = `scale(${scale}) translate(${(0.5 - ink.centerX) * canvasW}px, ${(0.5 - ink.centerY) * canvasH}px)`;
}

/** Artwork drawn as the Q tiles draw it: its ink in one color, counters cut
 * through to the surface beneath. Without a URL it draws `fallback`. With
 * `fillInk`, the ink is scaled to fill the box, for artwork whose canvas has
 * wide empty margins. */
export function Artwork(props: { src: string | undefined; class?: string; size?: number; color: string; fallback?: JSX.Element; fillInk?: boolean }) {
  return (
    <Show when={props.src} fallback={props.fallback}>
      {(src) => (
        <span
          ref={(element: HTMLElement) => {
            if (props.fillInk && props.src !== undefined) void fitInk(element, props.src);
          }}
          class={`block shrink-0 ${props.class ?? ''}`}
          style={{
            ...(props.size ? { width: `${props.size}px`, height: `${props.size}px` } : {}),
            '-webkit-mask': `url(${src()}) center / contain no-repeat`,
            mask: `url(${src()}) center / contain no-repeat`,
            'background-color': props.color,
          }}
        />
      )}
    </Show>
  );
}

/** A word on a plate in the ink, letters cut through, as the stock Color and
 * Sharp icons are; a badge letter on its lower right corner. */
export function Wordmark(props: { word: string; badge?: string; color: string }) {
  return (
    <span class="relative flex h-[30px] w-[39px] items-center justify-center overflow-visible rounded-[3px]" style={{ background: props.color }}>
      <span
        class="inline-block text-[22px] leading-none font-bold tracking-[-0.03em] whitespace-nowrap"
        style={{ color: 'rgb(0 0 0 / 0.88)', transform: 'scaleX(0.46)' }}
      >
        {props.word}
      </span>
      <Show when={props.badge}>
        <span
          class="absolute -right-[8px] -bottom-[6px] flex h-[16px] w-[16px] items-center justify-center rounded-[3px] text-[12px] leading-none font-bold"
          style={{ background: props.color, color: '#000', 'box-shadow': '0 0 0 2px #000' }}
        >
          {props.badge}
        </span>
      </Show>
    </span>
  );
}

/** One to four letters in an outlined tile: a film icon of our own letters. */
export function Monogram(props: { text: string; size: number; color: string }) {
  const code = () => props.text || 'Aa';
  return (
    <span
      class="flex shrink-0 items-center justify-center rounded-[18%] font-bold"
      style={{
        width: `${props.size}px`,
        height: `${props.size}px`,
        'box-shadow': `inset 0 0 0 ${props.size / 12}px ${props.color}`,
        color: props.color,
        'font-size': `${props.size * [0.5, 0.5, 0.46, 0.36, 0.28][code().length]}px`,
        'font-variant': 'small-caps',
        'font-stretch': 'condensed',
        'letter-spacing': '-0.02em',
        'text-transform': 'none',
        opacity: props.text ? 1 : 0.6,
      }}
    >
      {code()}
    </span>
  );
}
