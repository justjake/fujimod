import type { Firmware } from '../catalog';
import { fetchFirmware } from './fetch-firmware';
import { createSignal, onSettled } from 'solid-js';
import { initialize } from 'fujimod';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import { identify } from '../recovery/identify';
import { CameraStore, type IdentifiedFirmware, type LibraryFirmware } from './store';

/** Browser-local firmware selection shared by the launcher and recovery tools. */
export function createFirmwareLibrary(identifyFile?: (file: File) => Promise<IdentifiedFirmware>) {
  const store = new CameraStore();
  const [entries, setEntries] = createSignal<LibraryFirmware[]>([]);
  const [selected, setSelected] = createSignal<string>();
  const [ready, setReady] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal('');
  const chosen = () => entries().find((entry) => entry.sha256 === selected());
  const report = (caught: unknown) => setError(shownError(caught));
  async function refresh() {
    const library = await store.library();
    setEntries(library);
    return library;
  }
  async function add(file?: File) {
    if (!file || busy()) return;
    setBusy(true);
    setError('');
    try {
      const firmware = await (identifyFile
        ? identifyFile(file)
        : (async () => {
            await initialize();
            return identify(new Uint8Array(await file.arrayBuffer()));
          })());
      const entry = await store.addFirmware(file, firmware);
      await refresh();
      setSelected(entry.sha256);
    } catch (caught) {
      report(caught);
    } finally {
      setBusy(false);
    }
  }
  async function get(firmware: Firmware): Promise<LibraryFirmware> {
    const saved = entries().find((entry) => entry.sha256 === firmware.sha256);
    if (saved) {
      setSelected(saved.sha256);
      return saved;
    }
    if (busy()) throw Error('Firmware import is already in progress.');
    setBusy(true);
    setError('');
    try {
      const file = await fetchFirmware(firmware);
      const entry = await store.addFirmware(file, firmware);
      await refresh();
      setSelected(entry.sha256);
      return entry;
    } catch (caught) {
      report(caught);
      throw caught;
    } finally {
      setBusy(false);
    }
  }
  async function remove(sha256: string) {
    if (busy()) return;
    setBusy(true);
    setError('');
    try {
      await store.removeFirmware(sha256);
      const library = await refresh();
      if (selected() === sha256) setSelected(library[0]?.sha256);
    } catch (caught) {
      report(caught);
    } finally {
      setBusy(false);
    }
  }
  onSettled(() => {
    let disposed = false;
    const load = () =>
      store
        .library()
        .then((library) => {
          if (disposed) return;
          setEntries(library);
          setSelected((current) =>
            library.some((entry) => entry.sha256 === current) ? current : library[0]?.sha256,
          );
        }, report)
        .finally(() => {
          if (!disposed) setReady(true);
        });
    void load();
    const visible = () => {
      if (document.visibilityState === 'visible') void load();
    };
    window.addEventListener('focus', load);
    document.addEventListener('visibilitychange', visible);
    return () => {
      disposed = true;
      window.removeEventListener('focus', load);
      document.removeEventListener('visibilitychange', visible);
    };
  });
  return {
    store,
    entries,
    selected,
    chosen,
    ready,
    busy,
    error,
    select: setSelected,
    add,
    get,
    remove,
  };
}
export interface FirmwareLibrary {
  store: CameraStore;
  entries(): LibraryFirmware[];
  selected(): string | undefined;
  chosen(): LibraryFirmware | undefined;
  ready(): boolean;
  busy(): boolean;
  error(): string;
  select(sha256: string | undefined): void;
  add(file?: File): Promise<void>;
  get(firmware: Firmware): Promise<LibraryFirmware>;
  remove(sha256: string): Promise<void>;
}
