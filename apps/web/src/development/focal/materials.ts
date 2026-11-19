import type { JSX } from '@solidjs/web';

/* The editors' materials. Each names a CSS variable the page's look (Glass,
   High contrast, Fujimod menu) defines, so the custom settings editor and the
   film editor share one design language. */

export const ACCENT = 'var(--m-accent)';
export const TEXT = 'text-[24px] leading-none';
export const SMALL = 'text-[21px] leading-none';

/** A chip over the picture: key hints, name cards. */
export const GLASS: JSX.CSSProperties = {
  background: 'var(--m-chip)',
  'box-shadow': 'var(--m-chip-edge)',
  'backdrop-filter': 'var(--m-blur)',
  color: '#fff',
};

/** The column of rows down the left edge. */
export const COLUMN: JSX.CSSProperties = {
  background: 'var(--m-column)',
  'box-shadow': 'var(--m-column-edge)',
  'backdrop-filter': 'var(--m-blur)',
  color: '#fff',
};

/** A row's card; the focused one brighter and rimmed. */
export const TRACK = (focused: boolean): JSX.CSSProperties => ({
  background: focused ? 'var(--m-focus)' : 'var(--m-track)',
  'border-radius': '13px',
  'box-shadow': focused ? 'var(--m-focus-edge)' : 'var(--m-track-edge)',
});

/** The recessed strip a card's choices or scale run in. */
export const WELL: JSX.CSSProperties = {
  background: 'var(--m-well)',
  'box-shadow': 'var(--m-well-edge)',
};
