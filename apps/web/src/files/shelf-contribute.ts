import { contribute } from './contribute';
import type { Contribute } from './ContributeDialog';
import { shelfManifest, type ShelfFile } from './shelf';

/** Uploads `files` as one contribution; retries from the same dialog resume it. */
export function shelfContribute(files: readonly ShelfFile[]): Contribute {
  const id = crypto.randomUUID();
  return async (message, progress) => {
    await contribute(
      {
        id,
        files: files.map((file) => ({
          model: file.camera.model,
          serial: file.camera.serial,
          name: file.name,
          blob: file.blob,
        })),
        manifest: shelfManifest(files),
        message,
      },
      ({ done, total }) => progress(total ? done / total : 1),
    );
    return { id };
  };
}
