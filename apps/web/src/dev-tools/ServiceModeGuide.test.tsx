import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, beforeAll, expect, test, vi } from 'vite-plus/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { initialize } from 'fujimod';
import type { ToolSession } from './camera-model';
import { Wizard } from './routes';
import { Workbench } from './Workbench';
import { createTestCameraModel, testCamera } from './test-camera-model';

beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(resolve('../../bindings/javascript/pkg/fujimod_bg.wasm')),
  });
});

afterEach(() => vi.unstubAllGlobals());

function page(debug?: Partial<ToolSession['debug']>, initialWizard?: Wizard) {
  const host = document.createElement('div');
  document.body.append(host);
  const next = vi.fn(() =>
    testCamera('X70', {
      identity: { product: 'X70', internalVersion: '1.39' },
      debugMode: 'on',
    }),
  );
  const test = createTestCameraModel({
    next,
    session: () => ({ debug: debug as ToolSession['debug'] }),
  });
  const dispose = render(
    () => <Workbench model={test.model} usbSupported initialWizard={initialWizard} />,
    host,
  );
  const button = (name: string) =>
    [...host.querySelectorAll('button')].find(
      (node) => (node.getAttribute('aria-label') ?? node.textContent?.trim()) === name,
    );
  return {
    host,
    next,
    button,
    dispose,
    unplug: () => test.update('X70', (camera) => ({ ...camera, present: false })),
  };
}

function workflowSession() {
  const table = JSON.stringify({
    firmware_id: 'x70-110',
    firmware_sha256: 'a'.repeat(64),
  });
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async (url: URL) =>
        new Response(
          url.pathname.endsWith('camera.json')
            ? table
            : JSON.stringify({
                schema: 1,
                firmwares: [
                  {
                    id: 'x70-110',
                    body: 'x70',
                    model: 'X70',
                    version: '1.10',
                    sha256: 'a'.repeat(64),
                    bytes: 42,
                    download_url: 'https://example.org/firmware',
                    camera_table: 'runtime/x70-110/camera.json',
                  },
                ],
              }),
        ),
    ),
  );
  const session = {
    enterDebugMode: vi.fn(async () => ({
      alreadyPersisted: false,
      progress: ['Debug mode enabled'],
    })),
    leaveDebugMode: vi.fn(async () => ({
      alreadyPersisted: false,
      progress: ['Debug mode disabled'],
    })),
  };
  return { session, table };
}

test('a guide deep link opens directly to its instructions', () => {
  const p = page(undefined, Wizard.Service);
  try {
    flush();
    expect(p.button('Begin')).toBeTruthy();
    expect(p.host.querySelector('.td-enter')).toBeNull();
  } finally {
    p.dispose();
  }
});

test('Disable debug mode persists debug access off for the camera firmware', async () => {
  const { session, table } = workflowSession();
  const p = page(session);
  try {
    flush();
    p.button('Show instructions')!.click();
    flush();
    p.button('Begin')!.click();
    flush();
    p.host.querySelector<HTMLInputElement>('.dm-check input')!.click();
    flush();
    p.button('Allow camera access')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Open workbench')).toBeTruthy();
    });
    p.button('Open workbench')!.click();
    flush();
    p.button('Disable debug mode')!.click();
    flush();
    p.host.querySelector<HTMLButtonElement>('.dm-huge')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.host.textContent).toContain('Debug mode is off.');
    });
    expect(session.leaveDebugMode).toHaveBeenCalledWith(table, 'x70-110', 'a'.repeat(64));
  } finally {
    p.dispose();
  }
});

test('the service guide connects without selecting firmware or writing anything', async () => {
  const { session } = workflowSession();
  const p = page(session);
  try {
    flush();
    p.button('Show instructions')!.click();
    flush();
    p.button('Begin')!.click();
    flush();
    expect(p.host.querySelector('.dm-camera')).toBeNull();
    expect(p.host.querySelector('.dpad .u.glow')).not.toBeNull();
    expect(p.host.querySelector('.slide.full')).not.toBeNull();
    expect(p.host.querySelector('[aria-label="USB connected"]')).not.toBeNull();
    expect(p.host.textContent).toContain('do not use the OFF switch');
    p.host.querySelector<HTMLInputElement>('.dm-check input')!.click();
    flush();
    p.button('Allow camera access')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Open workbench')).toBeTruthy();
    });
    p.button('Open workbench')!.click();
    flush();
    expect(p.host.querySelector('.wb-camera h2')!.textContent).toBe('X70');
    expect(session.enterDebugMode).not.toHaveBeenCalled();
    expect(session.leaveDebugMode).not.toHaveBeenCalled();
  } finally {
    p.dispose();
  }
});
