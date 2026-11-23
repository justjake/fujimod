import { For, Show, createMemo, createSignal, onSettled, untrack } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import { ObjectFields, type FieldContext } from './SchemaFields';
import { documentText, filled, type DocumentLocation, type JsonValue, type Schema } from './schema';
import {
  chooseSdCard,
  folderAccessSupported,
  listFiles,
  readText,
  regrant,
  rememberedSdCard,
  removeFile,
  writeText,
  type SdCard,
} from './sd-card';
import { documentSchemas } from './schemas';
import { problems } from './validation';
import './documents.css';

/** Where the open document came from, and so where Save puts it. */
type Source =
  | { kind: 'sd-card'; name: string }
  | { kind: 'file'; name: string }
  | { kind: 'new'; name: string };

const extensionsOf = (location: DocumentLocation) =>
  location.extensions ??
  (location.name ? [location.name.slice(location.name.lastIndexOf('.'))] : []);

/** The lowest free name for a new file: `#` runs become a number. */
function freeName(location: DocumentLocation, taken: readonly string[]) {
  const pattern = location.new_name ?? 'NEW#.JSN';
  const width = pattern.match(/#+/)?.[0].length ?? 1;
  const used = new Set(taken.map((name) => name.toUpperCase()));
  for (let number = 1; ; number++) {
    const name = pattern.replace(/#+/, String(number).padStart(width, '0'));
    if (!used.has(name.toUpperCase())) return name;
  }
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** The editor for every document of one schema: the one file or the folder
 * of files its `x-document` names, on a granted SD card or opened from disk. */
export function documentEditor(schema: Schema) {
  return () => <DocumentEditor schema={schema} />;
}

function DocumentEditor(props: { schema: Schema }) {
  // One page edits one schema for its whole life.
  const schema = untrack(() => props.schema);
  const location = schema['x-document']!;
  const single = !!location.name;
  const locked = schema['x-locked-by'];
  const [card, setCard] = createSignal<SdCard>();
  const [supported, setSupported] = createSignal(false);
  const [files, setFiles] = createSignal<string[]>([]);
  const [references, setReferences] = createSignal<Record<string, string[]>>({});
  const [source, setSource] = createSignal<Source>();
  const [value, setValue] = createSignal<JsonValue>(filled(undefined, schema));
  const [saved, setSaved] = createSignal<string>();
  const [error, setError] = createSignal('');
  const [status, setStatus] = createSignal('');
  let picker!: HTMLInputElement;

  const text = createMemo(() => documentText(value(), schema));
  // Checked as saved, so unset values are missing rather than null.
  const found = createMemo(() => problems(JSON.parse(text()) as JsonValue, schema));
  const changed = () => text() !== saved();
  const isLocked = () =>
    !!locked &&
    typeof value() === 'object' &&
    (value() as Record<string, JsonValue>)?.[locked] === true;

  const context: FieldContext = {
    problems: found,
    references: (id) => references()[id],
    disabled: () => isLocked(),
    lock: locked && `/${locked}`,
  };

  const load = (raw: string, from: Source) => {
    try {
      const parsed = JSON.parse(raw) as JsonValue;
      setValue(filled(parsed, schema));
      setSource(from);
      setSaved(documentText(filled(parsed, schema), schema));
      setError('');
    } catch (caught) {
      setError(`${from.name} is not a valid document: ${message(caught)}`);
    }
  };

  const fresh = (name: string) => {
    setValue(filled(undefined, schema));
    setSource({ kind: 'new', name });
    setSaved(undefined);
    setError('');
  };

  const refresh = async (granted: SdCard) => {
    if (!single) setFiles(await listFiles(granted, location.directory, extensionsOf(location)));
    const lists: Record<string, string[]> = {};
    for (const other of documentSchemas) {
      const where = other['x-document'];
      if (where?.extensions)
        lists[where.id] = await listFiles(granted, where.directory, where.extensions);
    }
    setReferences(lists);
  };

  const openOnCard = async (granted: SdCard, name: string) => {
    const raw = await readText(granted, location.directory, name);
    if (raw === undefined) fresh(name);
    else load(raw, { kind: 'sd-card', name });
  };

  const use = async (granted: SdCard) => {
    setCard(granted);
    if (!granted.granted) return;
    try {
      await refresh(granted);
      if (single) await openOnCard(granted, location.name!);
    } catch (caught) {
      setError(message(caught));
    }
  };

  onSettled(() => {
    setSupported(folderAccessSupported());
    if (!single) fresh(freeName(location, []));
    else fresh(location.name!);
    void rememberedSdCard().then((remembered) => remembered && use(remembered));
  });

  const connect = async () => {
    try {
      const current = card();
      await use(current && !current.granted ? await regrant(current) : await chooseSdCard());
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === 'AbortError'))
        setError(message(caught));
    }
  };

  const openFile = async (file: File) => load(await file.text(), { kind: 'file', name: file.name });

  const download = () => {
    const name = source()?.name ?? location.name ?? freeName(location, []);
    const link = document.createElement('a');
    link.href = URL.createObjectURL(new Blob([text()], { type: 'application/json' }));
    link.download = name;
    link.click();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
    setSaved(text());
  };

  const save = async () => {
    const granted = card();
    const from = source();
    if (!granted?.granted || !from || from.kind === 'file') return download();
    try {
      await writeText(granted, location.directory, from.name, text());
      setSaved(text());
      setSource({ kind: 'sd-card', name: from.name });
      setStatus(`Saved ${from.name}`);
      setTimeout(() => setStatus(''), 2000);
      await refresh(granted);
    } catch (caught) {
      setError(message(caught));
    }
  };

  const duplicate = () => {
    const copy = value() as Record<string, JsonValue>;
    setValue(locked ? { ...copy, [locked]: false } : { ...copy });
    setSource({ kind: 'new', name: freeName(location, files()) });
    setSaved(undefined);
  };

  const remove = async () => {
    const granted = card();
    const from = source();
    if (!granted?.granted || from?.kind !== 'sd-card') return;
    if (!confirm(`Delete ${from.name} from the SD card?`)) return;
    try {
      await removeFile(granted, location.directory, from.name);
      await refresh(granted);
      fresh(freeName(location, files()));
    } catch (caught) {
      setError(message(caught));
    }
  };

  const onCard = () => !!card()?.granted;
  const saveLabel = () => (onCard() && source()?.kind !== 'file' ? 'Save' : 'Download');

  return (
    <SiteFrame class="documents">
      <div class={{ 'doc-layout': true, 'doc-folder': !single }}>
        <header class="doc-header">
          <h1>{schema.title}</h1>
          <div class="doc-actions">
            <Show when={supported()}>
              <button
                type="button"
                class={{ 'doc-card-state': true, connected: onCard() }}
                onClick={connect}
              >
                {onCard()
                  ? `SD card: ${card()!.root.name}`
                  : card()
                    ? 'Reconnect SD card'
                    : 'Use SD card'}
              </button>
            </Show>
            <button type="button" onClick={() => picker.click()}>
              Open file
            </button>
            <input
              ref={(element) => (picker = element)}
              type="file"
              hidden
              accept={[...extensionsOf(location), '.json'].join(',')}
              onChange={(event) => {
                const file = event.currentTarget.files?.[0];
                if (file) void openFile(file);
                event.currentTarget.value = '';
              }}
            />
          </div>
        </header>
        <Show when={error()}>
          <p class="doc-error" role="alert">
            {error()}
          </p>
        </Show>
        <Show when={!single}>
          <nav class="doc-files" aria-label="Files">
            <button
              type="button"
              class="doc-new"
              onClick={() => fresh(freeName(location, files()))}
            >
              New
            </button>
            <ul>
              <For each={files()}>
                {(name) => (
                  <li>
                    <button
                      type="button"
                      aria-current={
                        source()?.kind === 'sd-card' && source()?.name === name ? 'true' : undefined
                      }
                      onClick={() => void openOnCard(card()!, name)}
                    >
                      {name}
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </nav>
        </Show>
        <main class="doc-editor">
          <div class="doc-file-bar">
            <span class="doc-file-name">{source()?.name}</span>
            <span class="doc-status" role="status">
              {status()}
            </span>
            <Show when={!single && onCard() && source()?.kind === 'sd-card'}>
              <button type="button" onClick={duplicate}>
                Duplicate
              </button>
              <button type="button" class="doc-delete" disabled={isLocked()} onClick={remove}>
                Delete
              </button>
            </Show>
            <button
              type="button"
              class="doc-save"
              disabled={found().size > 0 || (saveLabel() === 'Save' && !changed())}
              onClick={save}
            >
              {saveLabel()}
            </button>
          </div>
          <ObjectFields
            key=""
            schema={schema}
            value={value()}
            pointer=""
            set={setValue}
            context={context}
            depth={0}
          />
        </main>
      </div>
    </SiteFrame>
  );
}
