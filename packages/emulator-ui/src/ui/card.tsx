import { For, createSignal, createEffect, createMemo, untrack } from 'solid-js';
import type { Engine } from '../engine/api';
import type { CardEntry, CardState } from 'fujimod/emulator';
import { zipBlob } from '../archive';
import { flags } from '../flags';
import { FileList, type FileListItem } from './file-list';
import { downloadBlob } from './download';

function downloadable(entry: CardEntry) {
  return flags.allowSdCardBootDownload || !/^ff(?:boot|sys)\.bin$/i.test(entry.name);
}

function visibleEntries(entries: CardEntry[]): CardEntry[] {
  return entries
    .filter((entry) => entry.kind === 'directory' || downloadable(entry))
    .map((entry) =>
      entry.kind === 'directory'
        ? { ...entry, children: visibleEntries(entry.children ?? []) }
        : entry,
    );
}

function listItems(entries: readonly CardEntry[]): FileListItem[] {
  return entries.map((entry) =>
    entry.kind === 'directory'
      ? { key: entry.path, name: entry.name, children: listItems(entry.children ?? []) }
      : { key: entry.path, name: entry.name, bytes: entry.bytes },
  );
}

/** All observations/downloads are bound to the engine and the card revision that supplied them. */
export function CardBrowser(props: {
  engine?: Engine;
  present: (value: boolean) => Promise<void>;
  format: () => Promise<void>;
}) {
  const [state, setState] = createSignal<CardState>();
  const [tree, setTree] = createSignal<{ revision: number; entries: CardEntry[] }>();
  const entries = createMemo(() => visibleEntries(tree()?.entries ?? []));
  const files = createMemo(() => {
    const result: CardEntry[] = [];
    const visit = (items: CardEntry[]) => {
      for (const item of items)
        if (item.kind === 'directory') visit(item.children ?? []);
        else result.push(item);
    };
    visit(entries());
    return result;
  });
  const [status, setStatus] = createSignal('');
  let generation = 0,
    observation = 0;
  createEffect(
    () => props.engine,
    (engine) => {
      const current = ++generation;
      setState(undefined);
      setTree(undefined);
      setStatus('');
      let previous: CardState | undefined, pendingRevision: number | undefined;
      const stop = engine?.cardChanges((state) => {
        if (
          state.supported !== previous?.supported ||
          state.present !== previous?.present ||
          state.revision !== previous?.revision
        )
          ++observation;
        previous = state;
        const observed = observation;
        setState(state);
        setStatus(state.busy ? 'Camera is writing…' : '');
        if (!state.supported || !state.present) {
          setTree(undefined);
          return;
        }
        if (
          state.busy ||
          state.revision === null ||
          state.revision === untrack(tree)?.revision ||
          state.revision === pendingRevision
        )
          return;
        const revision = state.revision;
        pendingRevision = revision;
        void engine
          .cardTree(revision)
          .then((value) => {
            if (current !== generation || observed !== observation) return;
            if (value.revision !== revision) throw Error('Card revision is unavailable');
            setTree({ revision: value.revision, entries: value.entries });
          })
          .catch((error) => {
            if (current === generation && observed === observation) setStatus(String(error));
          })
          .finally(() => {
            if (pendingRevision === revision) pendingRevision = undefined;
          });
      });
      return () => {
        ++generation;
        stop?.();
      };
    },
  );
  async function perform(task: () => Promise<void>) {
    const current = generation;
    try {
      await task();
    } catch (error) {
      if (current === generation) setStatus(String(error));
    }
  }
  async function download(entry: CardEntry) {
    const engine = props.engine,
      current = generation,
      revision = tree()?.revision;
    if (!engine || revision === undefined || !downloadable(entry)) return;
    await perform(async () => {
      const bytes = await engine.readCardFile(entry.path, { revision });
      if (current === generation) downloadBlob(new Blob([bytes.slice().buffer]), entry.name);
    });
  }
  async function all() {
    const engine = props.engine,
      current = generation,
      listing = tree();
    const downloads = files();
    if (!engine || !listing || !downloads.length) return;
    await perform(async () => {
      if (downloads.reduce((sum, item) => sum + item.bytes, 0) > 64 * 1024 ** 2)
        throw Error('Card download exceeds the 64 MiB browser archive limit.');
      const files: { path: string; data: Blob }[] = [];
      for (const entry of downloads) {
        const bytes = await engine.readCardFile(entry.path, { revision: listing.revision });
        if (current !== generation) return;
        files.push({
          path: entry.path.replace(/^[A-Za-z]:[\\/]/, '').replace(/^\//, ''),
          data: new Blob([bytes.slice().buffer]),
        });
      }
      const zip = await zipBlob(files);
      if (current === generation) downloadBlob(zip, 'sd-card.zip');
    });
  }
  return (
    <div class="sd" id="card-section" aria-label="SD card" hidden={state()?.supported === false}>
      <div class="head" id="card-header">
        <b>SD CARD</b>
        <span id="card-capacity">
          {state()
            ? `FAT32 · ${Number((state()!.capacity_bytes / 1024 ** 3).toFixed(1))} GB`
            : 'FAT32'}
        </span>
        <div class="seg" role="group" aria-label="SD card inserted">
          <For each={[false, true]}>
            {(present) => (
              <button
                data-card-present={String(present)}
                class={{ on: (state()?.present ?? true) === present }}
                aria-pressed={(state()?.present ?? true) === present ? 'true' : 'false'}
                disabled={!state()?.supported || state()?.busy}
                onClick={() => void perform(() => props.present(present))}
              >
                {present ? 'ON' : 'OFF'}
              </button>
            )}
          </For>
        </div>
      </div>
      <FileList
        id="card-files"
        items={listItems(entries())}
        download={(item) => {
          const entry = files().find((file) => file.path === item.key);
          return entry && download(entry);
        }}
      />
      <div class="row" style={{ 'margin-top': '10px' }}>
        <button
          class="pill"
          id="card-download"
          disabled={!state()?.supported || !files().length}
          onClick={() => void all()}
        >
          Download all
        </button>
        <button
          class="pill"
          id="card-format"
          disabled={!state()?.supported}
          onClick={() => void perform(props.format)}
        >
          Format
        </button>
      </div>
      <span class="visually-hidden" id="card-status" role="status">
        {status()}
      </span>
    </div>
  );
}
