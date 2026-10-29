import { For, Show } from 'solid-js';
import type { AssetProgress } from './engine/download-progress';
const mib = (bytes: number) => (bytes / 1048576).toFixed(1);
const size = (bytes: number) =>
  bytes < 1048576 ? `${Math.ceil(bytes / 1024)} KiB` : `${mib(bytes)} MiB`;
/** Every loading step is one row with the same bar; waiting rows have an empty bar. */
export function LoadingAssets(props: { assets: readonly AssetProgress[] }) {
  return (
    <Show when={props.assets.length > 0}>
      <ul class="loading-assets" aria-label="Loading">
        <For each={props.assets} keyed={false}>
          {(asset) => (
            <li data-asset={asset().url} data-state={asset().status}>
              <span>{asset().label}</span>
              <span class="asset-size">
                {asset().status === 'failed'
                  ? 'Failed'
                  : asset().status === 'complete'
                    ? `✓${asset().loaded && asset().unit !== 'ticks' ? ` ${size(asset().loaded)}` : ''}`
                    : asset().status === 'loading' && asset().total
                      ? asset().unit === 'ticks'
                        ? `${Math.min(99, Math.floor((100 * asset().loaded) / asset().total!))}%`
                        : `${mib(asset().loaded)} / ${mib(asset().total!)} MiB`
                      : ''}
              </span>
              <progress
                max="100"
                value={
                  asset().status === 'complete'
                    ? 100
                    : asset().status === 'loading'
                      ? asset().total
                        ? Math.min(
                            asset().unit === 'ticks' ? 99 : 100,
                            (100 * asset().loaded) / asset().total!,
                          )
                        : undefined
                      : 0
                }
                aria-label={asset().label}
              />
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}
