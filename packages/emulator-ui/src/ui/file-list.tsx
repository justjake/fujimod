import { For, Show, createSignal } from 'solid-js';
import { Icon } from './icons';
import './file-list.css';

/** One row: a file, or a folder of files. */
export interface FileListItem {
  key: string;
  name: string;
  /** Folders may omit their size. */
  bytes?: number;
  children?: readonly FileListItem[];
}

export function formatBytes(bytes: number) {
  return bytes >= 1024 ** 2
    ? `${(bytes / 1024 ** 2).toFixed(1)} MB`
    : bytes >= 1024
      ? `${Number((bytes / 1024).toFixed(1))} KB`
      : `${bytes} B`;
}

interface Actions {
  /** Shown on files, and on folders when `folderActions` is set. */
  download?: (item: FileListItem) => Promise<void> | void;
  remove?: (item: FileListItem) => void;
  folderActions?: boolean;
  /** The key of a row to tint, such as the newest file. */
  highlight?: string;
  /** No folders in the list: file rows drop the folder arrow's space. */
  flat?: boolean;
}

function Row(props: Actions & { item: FileListItem }) {
  const [open, setOpen] = createSignal(true),
    [busy, setBusy] = createSignal(false);
  const folder = () => props.item.children !== undefined;
  const actions = () => !folder() || props.folderActions;
  async function download() {
    setBusy(true);
    try {
      await props.download!(props.item);
    } finally {
      setBusy(false);
    }
  }
  const tail = (
    <>
      <Show when={props.item.bytes !== undefined}>
        <span class="s">{formatBytes(props.item.bytes!)}</span>
      </Show>
      <Show when={actions() && props.download}>
        <button
          type="button"
          class="dl"
          aria-label={`Download ${props.item.name}`}
          disabled={busy()}
          onClick={() => void download()}
        >
          <Icon name="download" size={15} />
        </button>
      </Show>
      <Show when={actions() && props.remove}>
        <button
          type="button"
          class="dl rm"
          aria-label={`Remove ${props.item.name}`}
          onClick={() => props.remove!(props.item)}
        >
          <Icon name="trash" size={15} />
        </button>
      </Show>
    </>
  );
  return (
    <Show
      when={folder()}
      fallback={
        <div class={{ r: true, new: props.highlight === props.item.key }}>
          <Show when={!props.flat}>
            <span class="tw" />
          </Show>
          <Icon name={/\.(jpe?g|png|bmp)$/i.test(props.item.name) ? 'image' : 'file'} />
          <span class="n">{props.item.name}</span>
          {tail}
        </div>
      }
    >
      <div class="r folder">
        <button
          type="button"
          class="fold"
          aria-expanded={open() ? 'true' : 'false'}
          aria-label={props.item.name}
          onClick={() => setOpen((value) => !value)}
        >
          <span class={['tw', { closed: !open() }]}>
            <Icon name="chevron" size={13} />
          </span>
          <Icon name="folder" />
          <span class="n">{props.item.name}</span>
        </button>
        {tail}
      </div>
      <div class="kids" hidden={!open()}>
        <For each={props.item.children ?? []} keyed={(item) => item.key}>
          {(item) => (
            <Row
              item={item()}
              download={props.download}
              remove={props.remove}
              folderActions={props.folderActions}
              highlight={props.highlight}
            />
          )}
        </For>
      </div>
    </Show>
  );
}

/** The file tree shared by the SD card, Dev Tools' Files pane, the Contribute dialog and bug reports. */
export function FileList(
  props: Omit<Actions, 'flat'> & { items: readonly FileListItem[]; id?: string },
) {
  const flat = () => props.items.every((item) => item.children === undefined);
  return (
    <div class="tree" id={props.id}>
      <For each={props.items} keyed={(item) => item.key}>
        {(item) => (
          <Row
            item={item()}
            download={props.download}
            remove={props.remove}
            folderActions={props.folderActions}
            highlight={props.highlight}
            flat={flat()}
          />
        )}
      </For>
    </div>
  );
}
