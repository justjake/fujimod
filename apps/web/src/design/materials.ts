import type { JSX } from '@solidjs/web';

/* The editors' materials. Each names a CSS variable the page's look (Glass,
   High contrast, Fujimod menu) defines, so the custom settings editor and the
   film editor share one design language. */

export const ACCENT = 'var(--m-accent)';
/* Text sizes are the camera menus' own, in camera pixels; nothing is smaller
   than its key legends. */
/** Names on cards over the picture: the menus' group labels. */
export const LABEL = 'text-[30px] leading-none';
/** Section headings and values being set: the menus' group titles. */
export const HEADING = 'text-[28px] leading-none';
/** Choices, buttons and control names: the menus' title bars. */
export const TEXT = 'text-[24px] leading-none';
/** Second lines and summaries: the menus' detail lines. */
export const DETAIL = 'text-[23px] leading-none';
/** Key legends, scale numbers and readouts: the camera's key hints. */
export const SMALL = 'text-[21px] leading-none';

/** A chip over the picture: key hints, name cards. */
export const GLASS: JSX.CSSProperties = {
  background: 'var(--m-chip)',
  'box-shadow': 'var(--m-chip-edge)',
  color: '#fff',
};

/** The column of rows down the left edge. */
export const COLUMN: JSX.CSSProperties = {
  background: 'var(--m-column)',
  'box-shadow': 'var(--m-column-edge)',
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

/** The chosen or focused item inside a card: a lighter plate and a rim, its
 * ink unchanged (as iOS's segmented control and list selection), never an
 * inversion. */
export const SELECTED: JSX.CSSProperties = {
  background: 'var(--m-selected, rgb(255 255 255 / 0.24))',
  'box-shadow': 'var(--m-selected-edge, inset 0 0 0 2px rgb(255 255 255 / 0.85))',
};
