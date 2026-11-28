import type { JSX } from '@solidjs/web';

/* The editors' materials. Each names a CSS variable the page's look (Glass,
   High contrast, Fujimod menu) defines, so the custom settings editor and the
   film editor share one design language. Sizes are camera pixels. */

/* Type: four sizes, all capitals as on the camera. */
/** Screen and recipe titles. */
export const TITLE = 'text-title font-semibold';
/** Everything read in a row: names, choices, buttons and list entries. */
export const BODY = 'text-body';
/** A value being read or set: the body size, semibold, with even digits. */
export const VALUE = 'text-body font-semibold tabular-nums';
/** Section labels, scale numbers, key legends and second lines. */
export const CAPTION = 'text-caption';

/* Color meanings: each color says one thing. */
/** The menu's color (red in the shooting menu): the focus stripe and the dial's needle. */
export const ACCENT = 'var(--m-accent)';
/** Changed from what was saved or opened. */
export const EDITED = 'var(--m-edited)';
/** Secondary ink: labels and values of rows without focus. */
export const DIM = 'rgb(255 255 255 / 0.62)';

/* Geometry, on a 4 px grid. */
export const ROW = 48;
export const HEADER = 40;
export const MARGIN = 12;
/** Cards, panels and pop-ups; controls inside them use INNER. */
export const RADIUS = 12;
export const INNER = 8;

/* Motion: one ease, three durations. */
export const EASE = 'cubic-bezier(0.2, 0, 0, 1)';
/** Focus moving between rows. */
export const FOCUS_MS = 160;
/** A row or section growing or shrinking. */
export const EXPAND_MS = 200;
/** A pop-up or modal appearing. */
export const POP_MS = 150;

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
  'backdrop-filter': 'var(--m-column-blur, none)',
  '-webkit-backdrop-filter': 'var(--m-column-blur, none)',
  color: '#fff',
};

/** The one focus treatment: the lighter plate with the menu's stripe on its
 * left. Without focus a row is the page itself. */
export const TRACK = (focused: boolean): JSX.CSSProperties => ({
  background: focused ? 'var(--m-focus)' : 'transparent',
  'box-shadow': focused ? 'var(--m-focus-edge)' : 'none',
  transition: `background ${FOCUS_MS}ms ${EASE}, box-shadow ${FOCUS_MS}ms ${EASE}`,
});

/** The hairline under a row, inset to the text's left edge. */
export const HAIRLINE = `inset 0 -1px 0 var(--m-hairline)`;

/** The recessed strip a control's choices or scale run in. */
export const WELL: JSX.CSSProperties = {
  background: 'var(--m-well)',
  'box-shadow': 'var(--m-well-edge)',
};

/** The one raised material: pop-ups and modals. */
export const RAISED: JSX.CSSProperties = {
  background: 'var(--m-popup)',
  'box-shadow': 'var(--m-popup-edge), 0 12px 32px rgb(0 0 0 / 0.6)',
  'border-radius': `${RADIUS}px`,
  color: '#fff',
};

/** A pop-up or modal panel appearing from its anchor. */
export const APPEAR: JSX.CSSProperties = {
  animation: `fm-appear ${POP_MS}ms ${EASE} both`,
};

/** The chosen option of a multiple choice control (a strip's chosen cell, a
 * switch's option, a color chip): the selected plate with white text and
 * artwork, or with MARK COLOR CHOICES the look's mark plate and ink. Only
 * focus is ever outlined. */
export const CHOSEN: JSX.CSSProperties = {
  background: 'var(--m-chosen, var(--m-selected, rgb(255 255 255 / 0.24)))',
  'box-shadow': 'none',
  color: 'var(--m-chosen-ink, #fff)',
};
/** Ink on a chosen option. */
export const CHOSEN_INK = 'var(--m-chosen-ink, #fff)';

export const SELECTED: JSX.CSSProperties = {
  background: 'var(--m-selected, rgb(255 255 255 / 0.24))',
  'box-shadow': 'none',
};

/** A state that is on (FILM ONLY, A|B, LIVE): the chip inverted. */
export const ON: JSX.CSSProperties = {
  background: '#fff',
  color: '#000',
  'box-shadow': 'none',
};
