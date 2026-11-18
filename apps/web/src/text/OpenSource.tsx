import { For, NoHydration, Show } from 'solid-js';
import type { LicensedComponent } from './license-index';

const readFileSync = import.meta.env.SSR ? (await import('node:fs')).readFileSync : undefined;
const productLabels: Record<string, string> = {
  camera: 'Camera',
  website: 'Website',
  emulator: 'Emulator',
};

function components(): readonly LicensedComponent[] {
  if (!readFileSync) return [];
  const output = process.env.FUJIMOD_PRERENDER_OUTPUT || 'dist';
  try {
    return JSON.parse(readFileSync(`${output}/server/credits-components.json`, 'utf8'));
  } catch (error) {
    if (import.meta.env.DEV && error instanceof Error && 'code' in error && error.code === 'ENOENT')
      return [];
    throw error;
  }
}

export function OpenSource() {
  const entries = components();
  return (
    <NoHydration>
      <Show when={entries.length}>
        <table class="credits-components">
          <thead>
            <tr>
              <th scope="col">Package</th>
              <th scope="col">Version</th>
              <th scope="col">License</th>
              <th scope="col">Used in</th>
            </tr>
          </thead>
          <tbody>
            <For each={entries}>
              {(entry) => (
                <tr>
                  <td>
                    <a href={entry.source}>{entry.name}</a>
                  </td>
                  <td>{entry.version}</td>
                  <td>
                    <a href={entry.licensePath}>{entry.license}</a>
                  </td>
                  <td>
                    {entry.products.map((product) => productLabels[product] ?? product).join(', ')}
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </NoHydration>
  );
}
