import { byModel, type FirmwareProgress } from './camera-progress';

/** Every firmware's generated progress document in the catalog, read at build time. */
export const progressDocuments: FirmwareProgress[] = Object.values(
  import.meta.glob<FirmwareProgress>('../../../../catalog/progress/*/*/progress.generated.json', {
    eager: true,
    import: 'default',
  }),
);

/** Latest recorded bring-up progress per model. */
export const cameraProgress = byModel(progressDocuments);
