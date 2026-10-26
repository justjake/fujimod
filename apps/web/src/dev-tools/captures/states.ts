/** Camera states captured on every body. The person sets each one with the camera's controls. */
export interface CaptureState {
  id: string;
  name: string;
  /** The person holds a control for the whole read. */
  held?: boolean;
  /** Captured once per named instance, such as each menu page. */
  named?: boolean;
}

export const CAPTURE_STATES: readonly CaptureState[] = [
  { id: 'rec-idle', name: 'Still shooting' },
  { id: 'afs-half-press', name: 'AF-S half press', held: true },
  { id: 'afc-half-press', name: 'AF-C half press', held: true },
  { id: 'manual-focus', name: 'Manual focus' },
  { id: 'q-menu', name: 'Q menu' },
  { id: 'menu-page', name: 'Menu page', named: true },
  { id: 'movie-standby', name: 'Movie standby' },
  { id: 'movie-recording', name: 'Movie recording' },
  { id: 'playback', name: 'Playback' },
  { id: 'after-shot', name: 'After a shot' },
  { id: 'other', name: 'Other', named: true },
];
