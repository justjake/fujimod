import { render } from '@solidjs/web';
import { For, Show } from 'solid-js';
import type { PhysicalControls } from 'fujimod';
import { PowerComboKeys } from '@fujimod/emulator-ui/ui/power-combo';
import versions from '../../../../catalog/firmware-versions.generated.json';
import './power-combos.css';

const modelNames = new Map(
  versions.releases.map((release) => [
    release.product.replace(/[^a-z0-9]/gi, '').toLowerCase(),
    release.product,
  ]),
);
// These two models share one body control document.
modelNames.set('xq', 'XQ1 / XQ2');
function modelName(body: string): string {
  const model = modelNames.get(body);
  if (!model) throw new Error(`No catalog model name for ${body}`);
  return model;
}

const bodies = Object.values(
  import.meta.glob<PhysicalControls>('../../../../catalog/bodies/*/controls.json', {
    eager: true,
    import: 'default',
  }),
).sort((a, b) => a.body_id.localeCompare(b.body_id, 'en', { numeric: true }));

if (import.meta.env.DEV) {
  const host = document.getElementById('power-combos');
  if (!host) throw new Error('Missing power combo review root');
  render(
    () => (
      <>
        <h1>Power-on combos</h1>
        <For each={bodies}>
          {(body) => (
            <section class="phone" data-body={body.body_id}>
              <h2>{modelName(body.body_id)}</h2>
              <Show when={body.boot?.sd} fallback={<p>SD sequence not documented</p>}>
                <PowerComboKeys controls={body} modes={['sd', 'firmware_update']} />
              </Show>
            </section>
          )}
        </For>
      </>
    ),
    host,
  );
}
