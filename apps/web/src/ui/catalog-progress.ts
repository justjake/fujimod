import {
  byModel,
  withOverrides,
  type FirmwareProgress,
  type ProgressOverride,
} from './camera-progress';

/** Every firmware's generated progress document in the catalog, read at build time. */
export const progressDocuments: FirmwareProgress[] = Object.values(
  import.meta.glob<FirmwareProgress>('../../../../catalog/progress/*/*/progress.generated.json', {
    eager: true,
    import: 'default',
  }),
);

/** Latest recorded bring-up progress per model, with publishing overrides. */
export const cameraProgress = byModel(
  withOverrides(
    progressDocuments,
    Object.values(
      import.meta.glob<ProgressOverride>(
        '../../../../catalog/progress/*/*/progress.override.json',
        {
          eager: true,
          import: 'default',
        },
      ),
    ),
  ),
);
