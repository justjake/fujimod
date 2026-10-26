import { createEffect, createSignal, onCleanup } from 'solid-js';
import type { Camera, CameraLogEntry } from './camera-model';
import { exportLog } from './log';
import type { ShelfCamera, ShelfFile } from '../files/shelf';

/** The folder a camera's files go in: its model and serial once identified. */
export function shelfCamera(camera: Camera): ShelfCamera {
  return {
    id: camera.id,
    model: camera.identity?.product?.trim() || camera.name,
    serial: camera.identity?.serial?.trim() || camera.id,
    identity: { name: camera.name, transport: camera.transport, ...camera.identity },
  };
}

/**
 * Each camera's `log.jsonl`: its log entries after the last removal, as JSON
 * Lines without payload bytes. Rebuilt at most twice a second while the log grows.
 */
export function createLogFiles(
  log: () => readonly CameraLogEntry[],
  cameras: () => readonly Camera[],
) {
  const [cutoffs, setCutoffs] = createSignal<ReadonlyMap<string, number>>(new Map());
  const [files, setFiles] = createSignal<readonly ShelfFile[]>([]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const build = () => {
    timer = undefined;
    const entries = log();
    const after = cutoffs();
    setFiles(
      cameras().flatMap((camera) => {
        const own = entries.filter(
          (entry) => entry.camera === camera.id && entry.id > (after.get(camera.id) ?? -1),
        );
        if (!own.length) return [];
        const blob = new Blob([exportLog(own)], { type: 'application/jsonl' });
        return [{ key: `log:${camera.id}`, camera: shelfCamera(camera), name: 'log.jsonl', blob }];
      }),
    );
  };
  createEffect(
    () => [log(), cameras(), cutoffs()] as const,
    () => {
      timer ??= setTimeout(build, 500);
    },
  );
  onCleanup(() => clearTimeout(timer));
  return {
    files,
    isLog: (file: ShelfFile) => file.key.startsWith('log:'),
    /** Drops the entries logged so far for that camera; newer entries form a new file. */
    remove(camera: string) {
      const last = log()
        .filter((entry) => entry.camera === camera)
        .at(-1)?.id;
      if (last === undefined) return;
      setCutoffs((map) => new Map(map).set(camera, last));
      setFiles((all) => all.filter((file) => file.camera.id !== camera));
    },
  };
}
