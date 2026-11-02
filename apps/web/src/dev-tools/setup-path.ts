import stages from './bringup-stages.generated.json';

/** Bring-up camera stages the Get started steps complete, in the bring-up graph's order. */
export const SETUP_PATH = [
  'camera-backup',
  'camera-snapshot',
  'camera-debug',
  'camera-normal',
] as const;
export type SetupStage = (typeof SETUP_PATH)[number];

/** The step number Get started shows for a stage. */
export function stepNumber(stage: SetupStage): number {
  return SETUP_PATH.indexOf(stage) + 1;
}

/** The bring-up graph's camera stages: id, title and prerequisites. */
export const CAMERA_STAGES: readonly { id: string; title: string; requires: readonly string[] }[] =
  stages.stages;
