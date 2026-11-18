import { For, NoHydration, Show } from 'solid-js';
import type { LicensedComponent } from './license-index';

const readFileSync = import.meta.env.SSR ? (await import('node:fs')).readFileSync : undefined;
const products = [
  { id: 'camera', label: 'Camera' },
  { id: 'emulator', label: 'Emulator' },
  { id: 'website', label: 'Website' },
] as const;

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
        <ul class="credits-products">
          <For each={products}>
            {({ id, label }) => {
              const packages = entries.filter((entry) => entry.products.includes(id));
              return (
                <li>
                  <b>
                    {label} ({packages.length})
                  </b>{' '}
                  <For each={packages}>
                    {(entry, index) => (
                      <>
                        {index() > 0 ? ', ' : ''}
                        <a href={entry.source}>{entry.name}</a>
                      </>
                    )}
                  </For>
                </li>
              );
            }}
          </For>
        </ul>
        <details class="credits-license-details">
          <summary>Versions, licenses, and source</summary>
          <div
            class="credits-components-scroll"
            role="region"
            aria-label="Open source components"
            tabindex="0"
          >
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
                        {products
                          .filter(({ id }) => entry.products.includes(id))
                          .map(({ label }) => label)
                          .join(', ')}
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </details>
      </Show>
    </NoHydration>
  );
}
