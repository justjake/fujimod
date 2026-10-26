import { createSignal } from 'solid-js';
import type {
  Camera,
  CameraLogEntry,
  CameraModel,
  SessionHolder,
  ToolSession,
} from './camera-model';
import { Direction, Layer, Level } from './log';

/** An in-memory CameraModel for tests: cameras and sessions without USB. */
export function createTestCameraModel(options: {
  session?: (camera: string) => Partial<ToolSession>;
  next?: () => Camera;
}) {
  const [cameras, setCameras] = createSignal<readonly Camera[]>([]);
  const [log, setLog] = createSignal<readonly CameraLogEntry[]>([]);
  let nextLog = 0;
  let countdown: ((camera: string, message: string) => Promise<void>) | undefined;
  const sharingCalls: [string, boolean][] = [];
  function update(id: string, change: (camera: Camera) => Camera) {
    setCameras((list) => list.map((camera) => (camera.id === id ? change(camera) : camera)));
  }
  function append(entry: Omit<CameraLogEntry, 'id' | 'time'>) {
    setLog((entries) => [...entries, { ...entry, id: nextLog++, time: Date.now() }]);
  }
  const model: CameraModel = {
    cameras,
    log,
    async connectUsb() {
      if (!options.next) throw Error('No camera selected');
      const camera = options.next();
      setCameras((list) => [...list, camera]);
      append({
        camera: camera.id,
        layer: Layer.Connection,
        level: Level.Info,
        direction: Direction.Host,
        message: `Present · ${camera.name}`,
      });
    },
    async connectRemote() {
      throw Error('Remote cameras are not part of this test model');
    },
    async remove(id) {
      setCameras((list) => list.filter((camera) => camera.id !== id));
    },
    async withSession(id, tool, operation) {
      const camera = cameras().find((entry) => entry.id === id);
      if (!camera?.present) throw Error('Camera unplugged');
      if (camera.uncertain) throw Error('Reconnect the camera first');
      if (camera.session) throw Error('Camera in use');
      const holder: SessionHolder = { kind: 'local', tool };
      update(id, (current) => ({ ...current, session: holder }));
      try {
        return await operation(options.session?.(id) as ToolSession);
      } finally {
        // Like the USB model, re-read the debug flag after a debug-mode change.
        update(id, (current) => ({
          ...current,
          session: undefined,
          debugMode:
            tool === 'Enable debug mode'
              ? 'on'
              : tool === 'Disable debug mode'
                ? 'off'
                : current.debugMode,
        }));
      }
    },
    async setSharing(id, on) {
      sharingCalls.push([id, on]);
      update(id, (camera) => ({
        ...camera,
        sharing: on ? { link: `https://share.test/${id}`, peers: new Map() } : undefined,
      }));
    },
    onOwnerCountdown(run) {
      countdown = run;
      return () => {
        if (countdown === run) countdown = undefined;
      };
    },
    dismissProblem(id) {
      update(id, (camera) => ({ ...camera, problem: undefined }));
    },
    clearLog() {
      setLog([]);
    },
    dispose() {},
  };
  return {
    model,
    sharingCalls,
    add(camera: Camera) {
      setCameras((list) => [...list, camera]);
    },
    update,
    append,
    countdown: (camera: string, message: string) => countdown?.(camera, message),
  };
}

export function testCamera(id: string, overrides: Partial<Camera> = {}): Camera {
  return {
    id,
    transport: 'usb',
    name: id,
    present: true,
    uncertain: false,
    identity: { manufacturer: 'FUJIFILM', product: id, internalVersion: '1.66' },
    debugMode: 'off',
    ...overrides,
  };
}
