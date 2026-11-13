import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test } from 'vite-plus/test';
import generations from '../../devices/generations.generated.json';
import { started } from './camera-progress';
import { cameraProgress, progressDocuments } from './catalog-progress';
import { CameraTable } from './cameras';

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});

function homepageTable() {
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => <CameraTable columns={generations} progress={cameraProgress} owners={[]} />,
    host,
  );
  flush();
  return host;
}

test('the homepage reads every progress document in the catalog', () => {
  expect(progressDocuments.length).toBeGreaterThan(0);
  for (const document of progressDocuments)
    expect(cameraProgress[document.model], document.firmware_id).toBeDefined();
});

test('the homepage draws a progress row for every started model in the catalog', () => {
  const host = homepageTable();
  const rows = new Set(
    [...host.querySelectorAll('.progress-row .name')].map((name) => name.textContent),
  );
  const notStarted = new Set(
    [...host.querySelectorAll('.rest span')].map((name) => name.textContent),
  );
  const startedModels = Object.values(cameraProgress)
    .filter(started)
    .map((document) => document.model);
  expect(startedModels.length).toBeGreaterThan(0);
  for (const model of startedModels) {
    expect(rows, model).toContain(model);
    expect(notStarted, model).not.toContain(model);
  }
});

test('X-Processor 5 bodies with recorded progress are listed as started', () => {
  const xp5 = generations.flat().find((group) => group.name === 'X-Processor 5')!;
  const host = homepageTable();
  const recorded = xp5.models.filter((model) => cameraProgress[model]);
  expect(recorded).toEqual(expect.arrayContaining(['X-T5', 'X100VI', 'X-M5']));
  for (const model of recorded) {
    expect(started(cameraProgress[model]!), model).toBe(true);
    expect(host.querySelector(`[aria-label="${model} progress"]`), model).not.toBeNull();
  }
});

test('GFX bodies are grouped by their catalog processor', () => {
  const group = (name: string) => generations.flat().find((entry) => entry.name === name)!;
  expect(group('X-Processor Pro').models).toEqual(expect.arrayContaining(['GFX 50R', 'GFX 50S']));
  expect(group('X-Processor 4').models).toContain('GFX100S');
  expect(group('X-Processor 5').models).toContain('GFX100RF');
});

test('every started model has a processor group', () => {
  const listed = new Set(generations.flat().flatMap((group) => group.models));
  for (const document of Object.values(cameraProgress).filter(started))
    expect(listed, document.model).toContain(document.model);
});
