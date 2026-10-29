import { render } from '@solidjs/web';
import { createSignal, flush } from 'solid-js';
import { afterEach, beforeAll, expect, test, vi } from 'vite-plus/test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { initialize } from 'fujimod';
import type { ToolSession } from './camera-model';
import { Wizard } from './routes';
import { Workbench } from './Workbench';
import { GuideConnection, ServiceModeGuide } from './ServiceModeGuide';
import { createTestCameraModel, testCamera } from './test-camera-model';

beforeAll(async () => {
  await initialize({
    module_or_path: await readFile(resolve('../../bindings/javascript/pkg/fujimod_bg.wasm')),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

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
    session: () => ({
      debug: debug as ToolSession['debug'],
      memory: async () => ({
        read: async (_address, length) => new Uint8Array(length),
        write: async () => {},
        close: async () => {},
        overwritten: null,
      }),
    }),
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
    model: test.model,
    button,
    dispose() {
      dispose();
      host.remove();
    },
    unplug: () => test.update('X70', (camera) => ({ ...camera, present: false })),
  };
}

function workflowSession() {
  const table = JSON.stringify({
    firmware_id: 'x70-110',
    firmware_sha256: 'a'.repeat(64),
    setup: {},
    persistence: {},
  });
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async (url: URL) =>
        new Response(
          url.pathname.endsWith('service-workflow.generated.json')
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
                    service_workflow: 'runtime/x70-110/service-workflow.generated.json',
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
    p.button('Try to Connect')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Open workbench')).toBeTruthy();
    });
    p.button('Open workbench')!.click();
    // The bug-report scan holds the session briefly when the workbench opens.
    await vi.waitFor(() => {
      flush();
      expect(p.button('Disable debug mode')!.disabled).toBe(false);
    });
    p.button('Disable debug mode')!.click();
    flush();
    p.host.querySelector<HTMLButtonElement>('.dm-huge')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.host.textContent).toContain('Debug mode is off.');
    });
    expect(p.host.querySelector('.dm-ok')?.textContent).toContain(
      'Remove and reinsert the battery, then switch the camera on.',
    );
    expect(p.host.querySelector('.dm-ok .battery-cycle-icon')).not.toBeNull();
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
    expect(p.host.querySelector('[data-part="shutter"] [data-action="full"].on')).not.toBeNull();
    expect(p.host.querySelector('[aria-label="USB connected"]')).not.toBeNull();
    expect(p.host.textContent).toContain('do not use the OFF switch');
    expect(p.host.querySelector('.dm-entry li:first-child')?.textContent).toContain(
      'Switch the camera OFF, then remove and reinsert the battery.',
    );
    expect(p.host.querySelector('.dm-entry .battery-cycle-icon')).not.toBeNull();
    expect(p.host.textContent).not.toMatch(/green|lamp|LED/);
    expect(p.host.querySelector('.dm input[type=checkbox]')).toBeNull();
    const connect = vi.spyOn(p.model, 'connectUsb');
    p.button('Try to Connect')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Open workbench')).toBeTruthy();
    });
    const camera = p.model.cameras()[0];
    p.button('Open workbench')!.click();
    flush();
    expect(p.host.querySelector('.wb-camera h2')!.textContent).toBe('X70');
    expect(p.model.cameras()).toHaveLength(1);
    expect(p.model.cameras()[0].id).toBe(camera.id);
    expect(p.model.cameras()[0].present).toBe(true);
    expect(connect).toHaveBeenCalledOnce();
    expect(p.next).toHaveBeenCalledOnce();
    expect(session.enterDebugMode).not.toHaveBeenCalled();
    expect(session.leaveDebugMode).not.toHaveBeenCalled();
  } finally {
    p.dispose();
  }
});

test('canceling or failing to connect keeps the service instructions available for another try', async () => {
  const p = page();
  const connect = vi.spyOn(p.model, 'connectUsb');
  connect.mockRejectedValueOnce(new DOMException('No device selected', 'NotFoundError'));
  try {
    flush();
    p.button('Show instructions')!.click();
    flush();
    p.button('Begin')!.click();
    flush();
    p.button('Try to Connect')!.click();
    flush();
    expect(p.button('Try to Connect')!.disabled).toBe(true);
    expect(p.button('Open workbench')).toBeUndefined();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Try to Connect')!.disabled).toBe(false);
    });
    expect(p.host.textContent).toContain('Restart into service mode');
    expect(p.host.textContent).toContain('No device selected');
    expect(p.model.cameras()).toHaveLength(0);
    p.button('Try to Connect')!.click();
    await vi.waitFor(() => {
      flush();
      expect(p.button('Open workbench')).toBeTruthy();
    });
    expect(connect).toHaveBeenCalledTimes(2);
    expect(p.model.cameras()).toHaveLength(1);
  } finally {
    p.dispose();
  }
});

test('picker help appears after three seconds, resets on retry, and stops after connecting or leaving', () => {
  vi.useFakeTimers();
  const host = document.createElement('div');
  document.body.append(host);
  const [connection, setConnection] = createSignal(GuideConnection.Disconnected);
  const connect = vi.fn(() => setConnection(GuideConnection.Choosing));
  const dispose = render(
    () => <ServiceModeGuide connection={connection()} onConnect={connect} onClose={() => {}} />,
    host,
  );
  const button = (name: string) =>
    [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === name)!;
  try {
    flush();
    button('Begin').click();
    flush();
    button('Try to Connect').click();
    flush();
    expect(connect).toHaveBeenCalledOnce();
    expect(host.textContent).toContain('Restart into service mode');
    vi.advanceTimersByTime(2999);
    flush();
    expect(host.textContent).not.toContain('Can’t see your camera?');
    vi.advanceTimersByTime(1);
    flush();
    expect(host.textContent).toContain('Can’t see your camera?');
    expect(host.textContent).toContain('Our instructions might be wrong');
    setConnection(GuideConnection.Disconnected);
    flush();
    expect(button('Try to Connect').disabled).toBe(false);
    button('Try to Connect').click();
    flush();
    expect(host.textContent).not.toContain('Can’t see your camera?');
    expect(vi.getTimerCount()).toBe(1);
    setConnection(GuideConnection.Connected);
    flush();
    expect(host.textContent).toContain('Camera connected');
    expect(button('Open workbench')).toBeTruthy();
    expect(vi.getTimerCount()).toBe(0);
    setConnection(GuideConnection.Disconnected);
    flush();
    expect(host.textContent).toContain('Restart into service mode');
    button('Try to Connect').click();
    flush();
    expect(vi.getTimerCount()).toBe(1);
  } finally {
    dispose();
    host.remove();
  }
  expect(vi.getTimerCount()).toBe(0);
});

test('unsupported browsers cannot open the picker', () => {
  const host = document.createElement('div');
  const connect = vi.fn();
  const dispose = render(
    () => (
      <ServiceModeGuide
        connection={GuideConnection.Unsupported}
        onConnect={connect}
        onClose={() => {}}
      />
    ),
    host,
  );
  try {
    flush();
    host.querySelectorAll('button')[1].click();
    flush();
    const button = [...host.querySelectorAll('button')].find(
      (node) => node.textContent?.trim() === 'Try to Connect',
    )!;
    expect(button.disabled).toBe(true);
    button.click();
    expect(connect).not.toHaveBeenCalled();
    expect(host.textContent).toContain('Use Chrome or Edge.');
  } finally {
    dispose();
  }
});
