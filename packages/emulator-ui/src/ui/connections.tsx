import { For } from 'solid-js';
import type { Connection } from '../engine/api';
export function Connections(props: { rows: Connection[] }) {
  return (
    <section
      id="connections"
      class="connections"
      aria-label="External connections"
      hidden={!props.rows.length}
    >
      <details>
        <summary>Connections</summary>
        <ul>
          <For each={props.rows}>
            {(row) => (
              <li data-connection={row.id}>
                <b>{row.label}</b>
                <span>
                  {
                    { connected: 'Connected', disconnected: 'Disconnected', unknown: 'Unknown' }[
                      row.connection
                    ]
                  }{' '}
                  ·{' '}
                  {
                    { modeled: 'Modeled', partial: 'Partly modeled', unmodeled: 'Unmodeled' }[
                      row.support
                    ]
                  }
                </span>
                <small>{row.limits}</small>
              </li>
            )}
          </For>
        </ul>
      </details>
    </section>
  );
}
