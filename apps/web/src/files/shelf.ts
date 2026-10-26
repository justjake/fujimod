import { createSignal } from 'solid-js';
import type { FileListItem } from '@fujimod/emulator-ui/ui/file-list';

/** The camera a file came from: one folder per camera. */
export interface ShelfCamera {
  id: string;
  model: string;
  serial: string;
  /** Identity details recorded in the manifest. */
  identity?: Record<string, unknown>;
}

export interface ShelfFile {
  key: string;
  camera: ShelfCamera;
  name: string;
  blob: Blob;
  /** Descriptive facts for the manifest (addresses, hashes, tool settings); never bytes. */
  meta?: Record<string, unknown>;
}

/** How a tool adds its output for the selected camera. */
export type AddFile = (name: string, blob: Blob, meta?: Record<string, unknown>) => void;

export const folderName = (camera: ShelfCamera) => `${camera.model} ${camera.serial}`;

/** Lowercase `type-name` file names: `capture-AF-S half press` → `capture-af-s-half-press`. */
export function slug(text: string) {
  return (
    text
      .toLowerCase()
      .replace(/[^a-z0-9.]+/g, '-')
      .replace(/^-+|-+$/g, '') || 'file'
  );
}

/** Files collected from every tool during this page's life, until removed. */
export class FileShelf {
  private readonly state = createSignal<readonly ShelfFile[]>([]);
  private readonly latest = createSignal<string>();
  private next = 0;

  files = () => this.state[0]();
  newest = () => this.latest[0]();

  /** Adds a file; a name already used for that camera gets a numeric suffix. */
  add(camera: ShelfCamera, name: string, blob: Blob, meta?: Record<string, unknown>): ShelfFile {
    const taken = new Set(
      this.files()
        .filter((file) => file.camera.id === camera.id)
        .map((file) => file.name),
    );
    const dot = name.lastIndexOf('.');
    const [stem, extension] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ''];
    let unique = name;
    for (let count = 2; taken.has(unique); count++) unique = `${stem}-${count}${extension}`;
    const file = { key: `file-${++this.next}`, camera, name: unique, blob, meta };
    this.state[1]((files) => [...files, file]);
    this.latest[1](file.key);
    return file;
  }

  remove(key: string) {
    this.state[1]((files) => files.filter((file) => file.key !== key));
  }

  removeCamera(id: string) {
    this.state[1]((files) => files.filter((file) => file.camera.id !== id));
  }
}

/** One folder per camera, files in the order they were added. */
export function shelfItems(files: readonly ShelfFile[]): FileListItem[] {
  const folders = new Map<string, { camera: ShelfCamera; files: ShelfFile[] }>();
  for (const file of files) {
    const folder = folders.get(file.camera.id) ?? { camera: file.camera, files: [] };
    folder.files.push(file);
    folders.set(file.camera.id, folder);
  }
  return [...folders.values()].map(({ camera, files }) => ({
    key: `camera:${camera.id}`,
    name: folderName(camera),
    bytes: files.reduce((sum, file) => sum + file.blob.size, 0),
    children: files.map((file) => ({ key: file.key, name: file.name, bytes: file.blob.size })),
  }));
}

/** Describes every file for Download all and Contribute. */
export function shelfManifest(files: readonly ShelfFile[], createdAt = new Date()) {
  const cameras = new Map<string, { camera: ShelfCamera; files: ShelfFile[] }>();
  for (const file of files) {
    const entry = cameras.get(file.camera.id) ?? { camera: file.camera, files: [] };
    entry.files.push(file);
    cameras.set(file.camera.id, entry);
  }
  return {
    schema: 'fujimod-files-1',
    createdAt: createdAt.toISOString(),
    cameras: [...cameras.values()].map(({ camera, files }) => ({
      folder: folderName(camera),
      model: camera.model,
      serial: camera.serial,
      identity: camera.identity ?? {},
      files: files.map((file) => ({ name: file.name, bytes: file.blob.size, ...file.meta })),
    })),
  };
}
