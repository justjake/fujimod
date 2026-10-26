import { sessionState } from '@fujimod/emulator-ui/session/state';
import type { CameraRecord } from '../cameras/store';
import { createStore } from 'solid-js';
import { validateCatalog, type Catalog, type Firmware } from '../catalog';
import { validateIndex } from '../patch/catalog';
export interface PreviewCredit {
  title: string;
  author: string;
  url: string;
  authorUrl: string;
  license: string;
}
export type Release = ReturnType<typeof validateIndex>;
export interface CameraSupport {
  firmware: Firmware;
  mod: boolean;
  emu: boolean;
}
declare const __FUJIMOD_INITIAL_CATALOG__: Catalog;
declare const __FUJIMOD_INITIAL_RELEASE__: unknown;
export function supportedCameras(catalog: Catalog, release?: Release): CameraSupport[] {
  const entries = [...catalog.firmwares];
  for (const entry of release?.firmwares ?? [])
    if (!entries.some((known) => known.sha256 === entry.sha256)) entries.push(entry);
  return entries
    .filter((entry) => entry.emulate || release?.patches[entry.sha256])
    .map((firmware) => ({
      firmware,
      mod: !!release?.patches[firmware.sha256],
      emu: !!firmware.emulate,
    }));
}
export function createHomeState() {
  const catalog = validateCatalog(__FUJIMOD_INITIAL_CATALOG__);
  const release = __FUJIMOD_INITIAL_RELEASE__
    ? validateIndex(__FUJIMOD_INITIAL_RELEASE__)
    : undefined;
  return createStore({
    catalog,
    release,
    cameras: supportedCameras(catalog, release),
    savedCameras: [] as CameraRecord[],
    savedCamerasRead: false,
    activeCamera: undefined as CameraRecord | undefined,
    cameraBusy: false,
    storageMessage: '',
    storageError: false,
    ...sessionState(),
    firmware: undefined as Firmware | undefined,
    modSupported: false,
    modDisabled: true,
    modEnabled: true,
    bodyPreview: '',
    previewCredit: undefined as PreviewCredit | undefined,
  });
}
export type HomeState = ReturnType<typeof createHomeState>[0];
export type HomeSetter = ReturnType<typeof createHomeState>[1];
