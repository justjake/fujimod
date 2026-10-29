import controls from '../../../../catalog/bodies/x70/controls.json';
import type { PhysicalControls } from 'fujimod';

export const serviceModeControls = controls as PhysicalControls;
/** Service-mode entry, in order, around the shared control sequence. */
export const serviceModeEntry = [
  'Switch the camera OFF, then remove and reinsert the battery.',
  'Switch the camera ON while holding the buttons, then release them.',
];
