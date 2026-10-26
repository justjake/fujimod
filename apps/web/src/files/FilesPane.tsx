import { Show, createMemo, createSignal } from 'solid-js';
import { zipBlob } from '@fujimod/emulator-ui/archive';
import { downloadBlob } from '@fujimod/emulator-ui/ui/download';
import { FileList, formatBytes, type FileListItem } from '@fujimod/emulator-ui/ui/file-list';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { ContributeDialog, type Contribute } from './ContributeDialog';
import { folderName, shelfItems, shelfManifest, type ShelfFile } from './shelf';
import './files.css';

function stamp() {
  return new Date()
    .toISOString()
    .replaceAll(':', '-')
    .replace(/\.\d+Z$/, 'Z');
}

/** One zip: a folder per camera and a manifest describing every file. */
export async function filesArchive(files: readonly ShelfFile[]): Promise<Blob> {
  const manifest = new Blob([JSON.stringify(shelfManifest(files), null, 2)], {
    type: 'application/json',
  });
  return zipBlob([
    { path: 'manifest.json', data: manifest },
    ...files.map((file) => ({ path: `${folderName(file.camera)}/${file.name}`, data: file.blob })),
  ]);
}

/** Dev Tools' Files pane: everything the tools produced, with Download all and Contribute. */
export function FilesPane(props: {
  files: readonly ShelfFile[];
  newest?: string;
  onRemove: (file: ShelfFile) => void;
  onRemoveCamera: (camera: string) => void;
  contribute: (files: readonly ShelfFile[]) => Contribute;
}) {
  const [contributing, setContributing] = createSignal(false);
  const [failure, setFailure] = createSignal('');
  const items = createMemo(() => shelfItems(props.files));
  const total = () => props.files.reduce((sum, file) => sum + file.blob.size, 0);
  // The dialog uploads the files listed when it opened.
  const [shared, setShared] = createSignal<readonly ShelfFile[]>([]);
  const sharedItems = createMemo(() => shelfItems(shared()));
  const upload = createMemo(() => props.contribute(shared()));

  async function downloadItem(item: FileListItem) {
    const file = props.files.find((entry) => entry.key === item.key);
    if (file) return downloadBlob(file.blob, file.name);
    const camera = item.key.slice('camera:'.length);
    const files = props.files.filter((entry) => entry.camera.id === camera);
    if (files.length) await save(files, `${item.name} ${stamp()}.zip`);
  }
  async function save(files: readonly ShelfFile[], name: string) {
    setFailure('');
    try {
      downloadBlob(await filesArchive(files), name);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    }
  }
  return (
    <section class="files" aria-label="Files">
      <div class="files-head">
        <h3 class="wb-label">Files</h3>
        <Show when={props.files.length}>
          <span>
            {props.files.length} {props.files.length === 1 ? 'file' : 'files'} ·{' '}
            {formatBytes(total())}
          </span>
        </Show>
      </div>
      <div class="files-list">
        <Show when={props.files.length} fallback={<p class="files-empty">No files yet</p>}>
          <FileList
            items={items()}
            folderActions
            highlight={props.newest}
            download={downloadItem}
            remove={(item) => {
              const file = props.files.find((entry) => entry.key === item.key);
              if (file) props.onRemove(file);
              else props.onRemoveCamera(item.key.slice('camera:'.length));
            }}
          />
        </Show>
      </div>
      <Show when={failure()}>
        <p class="wb-status" role="alert">
          {failure()}
        </p>
      </Show>
      <div class="files-actions">
        <button
          type="button"
          class="wb-btn"
          disabled={!props.files.length}
          onClick={() => void save(props.files, `fujimod-files-${stamp()}.zip`)}
        >
          <Icon name="download" size={14} />
          Download all
        </button>
        <button
          type="button"
          class="wb-btn wb-primary"
          disabled={!props.files.length}
          onClick={() => {
            setShared(props.files);
            setContributing(true);
          }}
        >
          <Icon name="upload" size={14} />
          Contribute
        </button>
      </div>
      <ContributeDialog
        open={contributing()}
        items={sharedItems()}
        upload={(message, progress) => upload()(message, progress)}
        onClose={() => setContributing(false)}
      />
    </section>
  );
}
