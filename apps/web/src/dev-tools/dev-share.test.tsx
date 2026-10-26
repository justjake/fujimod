import { createSignal, flush } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
import type { Camera, CameraModel } from './camera-model';
import { startDevShare } from './dev-share';

class RegistrySocket {
  static OPEN = 1;
  static all: RegistrySocket[] = [];
  readyState = 0;
  onopen?: () => void;
  onmessage?: (event: { data: string }) => void;
  onerror?: () => void;
  sent: string[] = [];
  closed = false;
  constructor(readonly url: URL) {
    RegistrySocket.all.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
  }
  open() {
    this.readyState = RegistrySocket.OPEN;
    this.onopen?.();
  }
}
function camera(id: string, transport: Camera['transport'] = 'usb'): Camera {
  return {
    id,
    transport,
    name: id,
    present: true,
    uncertain: false,
    identity: { product: `Model ${id}`, serial: id },
  };
}
function fixture() {
  const [cameras, setCameras] = createSignal<readonly Camera[]>([]);
  const update = (id: string, patch: Partial<Camera>) =>
    setCameras((list) =>
      list.map((camera) => (camera.id === id ? { ...camera, ...patch } : camera)),
    );
  const share = vi.fn(async (id: string, on: boolean) => {
    update(id, {
      sharing: on ? { link: `https://example.com/#${id}`, peers: new Map() } : undefined,
    });
  });
  const model: CameraModel = {
    cameras,
    log: () => [],
    connectUsb: async () => {},
    connectRemote: async () => {},
    remove: async () => {},
    withSession: async () => {
      throw Error('unused');
    },
    setSharing: share,
    onOwnerCountdown: () => () => {},
    dismissProblem() {},
    clearLog() {},
    dispose() {},
  };
  return { model, share, update, setCameras };
}

test('auto-shares every USB camera independently and updates its registry entry', async () => {
  RegistrySocket.all = [];
  vi.stubGlobal('WebSocket', RegistrySocket);
  const f = fixture(),
    report = vi.fn();
  const stop = startDevShare(f.model, { report });
  try {
    f.setCameras([camera('one'), camera('two'), camera('remote', 'remote')]);
    flush();
    await Promise.resolve();
    flush();
    expect(f.share.mock.calls).toEqual([
      ['one', true],
      ['two', true],
    ]);
    expect(RegistrySocket.all).toHaveLength(2);
    for (const socket of RegistrySocket.all) {
      socket.open();
      expect(socket.url.pathname).toBe('/usb-proxy/dev-share');
      expect(['ws:', 'wss:']).toContain(socket.url.protocol);
      socket.onmessage?.({ data: '{"type":"registered"}' });
    }
    expect(JSON.parse(RegistrySocket.all[0].sent[0])).toEqual({
      url: 'https://example.com/#one',
      camera: { model: 'Model one', serial: 'one' },
      status: { online: true },
    });
    expect(report).toHaveBeenCalledWith('one', 'Development share registered');
    f.update('one', { present: false });
    flush();
    expect(JSON.parse(RegistrySocket.all[0].sent.at(-1)!)).toMatchObject({
      status: { online: false, reason: 'unplugged' },
    });
    expect(RegistrySocket.all[0].closed).toBe(false);
    await f.model.setSharing('two', false);
    flush();
    await Promise.resolve();
    flush();
    expect(f.share.mock.calls).toEqual([
      ['one', true],
      ['two', true],
      ['two', false],
    ]);
    expect(RegistrySocket.all[1].closed).toBe(true);
    f.update('two', { present: false });
    flush();
    f.update('two', { present: true });
    flush();
    await Promise.resolve();
    flush();
    expect(f.share.mock.calls.at(-1)).toEqual(['two', true]);
    expect(RegistrySocket.all).toHaveLength(3);
    window.dispatchEvent(new Event('pagehide'));
    expect(RegistrySocket.all.every((socket) => socket.closed)).toBe(true);
  } finally {
    stop();
    vi.unstubAllGlobals();
  }
});

test('registry reports use model-provided battery state and failures stay per camera', async () => {
  RegistrySocket.all = [];
  vi.stubGlobal('WebSocket', RegistrySocket);
  const f = fixture(),
    report = vi.fn();
  const stop = startDevShare(f.model, {
    report,
    status: () => ({ online: true, battery: 'FULL', battery_at_ms: 500 }),
  });
  try {
    f.setCameras([camera('one')]);
    flush();
    await Promise.resolve();
    flush();
    const socket = RegistrySocket.all[0];
    socket.open();
    expect(JSON.parse(socket.sent[0]).status).toEqual({
      online: true,
      battery: 'FULL',
      battery_at_ms: 500,
    });
    socket.onerror?.();
    expect(report).toHaveBeenCalledWith('one', 'Development share registry unavailable', true);
    socket.onmessage?.({ data: 'bad' });
    expect(socket.closed).toBe(true);
  } finally {
    stop();
    vi.unstubAllGlobals();
  }
});
