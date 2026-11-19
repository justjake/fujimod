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

/** Artwork drawn as the Q tiles draw it: its ink in one color, counters cut
 * through to the surface beneath. Without a URL it draws `fallback`. */
export function Artwork(props: { src: string | undefined; class?: string; size?: number; color: string; fallback?: JSX.Element }) {
  return (
    <Show when={props.src} fallback={props.fallback}>
      {(src) => (
        <span
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
