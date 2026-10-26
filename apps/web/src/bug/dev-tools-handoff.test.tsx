import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { beforeAll, expect, test, vi } from 'vite-plus/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { initialize } from 'fujimod';
import DevTools from '../dev-tools/Page';
import { createTestCameraModel, testCamera } from '../dev-tools/test-camera-model';

beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(resolve('../../bindings/javascript/pkg/fujimod_bg.wasm')),
  });
});

test('Dev Tools opened from a bug report shows the camera already in the page model', async () => {
  const test = createTestCameraModel({});
  test.add(testCamera('Selected camera'));
  const host = document.createElement('div');
  document.body.append(host);
  const dispose = render(() => <DevTools model={test.model} supported={true} />, host);
  try {
    await vi.waitFor(() => {
      flush();
      expect(host.querySelector('.wb-camera h2')?.textContent).toBe('Selected camera');
    });
    expect(
      [...host.querySelectorAll('button')].find(
        (node) => node.textContent?.trim() === 'Dump configuration',
      )!.disabled,
    ).toBe(false);
  } finally {
    dispose();
    host.remove();
  }
});
