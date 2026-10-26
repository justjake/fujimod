import { For, Show, createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import './firmware-downloads.css';

/** A search field appears once the list scrolls. */
export const SEARCH_THRESHOLD = 8;
const searchKey = (text: string) => text.toLowerCase().replace(/[^a-z0-9]/g, '');

export interface DownloadRow {
  model: string;
  processor?: string;
  /** The catalog's firmware version, when the catalog has this body. */
  version?: string;
  /** Fujifilm's firmware download for this body. */
  url: string;
  tag?: string;
}

/** Cameras, each with a link to its official Fujifilm firmware download. */
export function FirmwareDownloads(props: { heading: string; rows: readonly DownloadRow[] }) {
  const [query, setQuery] = createSignal('');
  const matches = () =>
    props.rows.filter((row) => searchKey(row.model).includes(searchKey(query())));
  return (
    <section class="downloads" aria-labelledby="downloads-heading">
      <h4 id="downloads-heading">{props.heading}</h4>
      <Show when={props.rows.length > SEARCH_THRESHOLD}>
        <label class="downloads-search">
          <Icon name="search" size={14} />
          <input
            type="search"
            placeholder="Search"
            aria-label="Search cameras"
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
        </label>
      </Show>
      <ul class="downloads-table">
        <For each={matches()}>
          {(row) => (
            <li class="downloads-camera">
              <span class="downloads-camera-name">
                {row.model}
                <Show when={row.processor}>
                  <span>{row.processor}</span>
                </Show>
              </span>
              <span class="downloads-camera-tag">
                <Show when={row.tag}>
                  <span class="downloads-tag">{row.tag}</span>
                </Show>
              </span>
              <span class="downloads-version">{row.version}</span>
              <a
                class="downloads-link"
                href={row.url}
                target="_blank"
                rel="noopener noreferrer"
                aria-label={`Download ${row.model} firmware from Fujifilm`}
              >
                Download
                <Icon name="download" size={13} />
              </a>
            </li>
          )}
        </For>
      </ul>
      <Show when={query() && matches().length === 0}>
        <p class="downloads-empty">No camera matches “{query()}”.</p>
      </Show>
    </section>
  );
}
