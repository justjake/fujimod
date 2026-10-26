import { createEffect, createRoot } from 'solid-js';
import type { Camera, CameraModel } from './camera-model';
import { holdReloadsDuringCameraSessions } from './reload-hold';

interface RegistrationStatus {
  online: boolean;
  reason?: string;
  battery?: string;
  battery_at_ms?: number;
}
export interface DevShareOptions {
  report(this: void, camera: string, message: string, error?: boolean): void;
  status?(camera: string): RegistrationStatus | undefined;
}
interface Registration {
  camera: Camera;
  socket?: WebSocket;
  payload?: string;
  wasPresent: boolean;
  wasSharing: boolean;
  pending: boolean;
  suspended: boolean;
}

/** One registry socket per USB camera; only the model starts and stops shares. */
export function startDevShare(model: CameraModel, options: DevShareOptions): () => void {
  const records = new Map<string, Registration>();
  let stopped = false;
  const close = (record: Registration) => {
    record.socket?.close();
    record.socket = undefined;
    record.payload = undefined;
  };
  const withdraw = () => {
    for (const record of records.values()) close(record);
  };
  const stopReloadHold = holdReloadsDuringCameraSessions(model, options.report);
  const registration = (camera: Camera) =>
    JSON.stringify({
      url: camera.sharing!.link,
      camera: { model: camera.identity?.product ?? null, serial: camera.identity?.serial ?? null },
      status: options.status?.(camera.id) ?? {
        online: camera.present,
        ...(!camera.present && { reason: 'unplugged' }),
      },
    });
  const open = (record: Registration) => {
    const url = new URL('/usb-proxy/dev-share', location.href);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(url);
    record.socket = socket;
    let announced = false;
    socket.onopen = () => {
      if (record.socket === socket && record.camera.sharing) {
        record.payload = registration(record.camera);
        socket.send(record.payload);
      }
    };
    socket.onmessage = ({ data }) => {
      if (record.socket !== socket) return;
      try {
        const reply: unknown = JSON.parse(String(data));
        if (
          !reply ||
          typeof reply !== 'object' ||
          !('type' in reply) ||
          reply.type !== 'registered'
        )
          throw Error('Development share registration failed');
        if (!announced) options.report(record.camera.id, 'Development share registered');
        announced = true;
      } catch (error) {
        options.report(record.camera.id, String(error), true);
        socket.close();
      }
    };
    socket.onerror = () => {
      if (record.socket === socket)
        options.report(record.camera.id, 'Development share registry unavailable', true);
    };
  };
  const dispose = createRoot((dispose) => {
    createEffect(
      () => model.cameras(),
      (cameras) => {
        if (stopped) return;
        const ids = new Set(cameras.map((camera) => camera.id));
        for (const [id, record] of records)
          if (!ids.has(id)) {
            close(record);
            records.delete(id);
          }
        for (const camera of cameras) {
          if (camera.transport !== 'usb') continue;
          let record = records.get(camera.id);
          if (!record) {
            record = {
              camera,
              wasPresent: false,
              wasSharing: false,
              pending: false,
              suspended: false,
            };
            records.set(camera.id, record);
          }
          record.camera = camera;
          if (!camera.present || !record.wasPresent) record.suspended = false;
          if (record.wasSharing && !camera.sharing && camera.present && record.wasPresent)
            record.suspended = true;
          record.wasPresent = camera.present;
          record.wasSharing = Boolean(camera.sharing);
          if (camera.sharing) {
            if (!record.socket) open(record);
            else if (record.socket.readyState === WebSocket.OPEN) {
              const payload = registration(camera);
              if (record.payload !== payload) {
                record.socket.send(payload);
                record.payload = payload;
              }
            }
          } else {
            close(record);
            if (camera.present && !camera.uncertain && !record.pending && !record.suspended) {
              record.pending = true;
              const current = record;
              void model
                .setSharing(camera.id, true)
                .catch((error: unknown) => {
                  current.suspended = true;
                  options.report(camera.id, String(error), true);
                })
                .finally(() => {
                  current.pending = false;
                });
            }
          }
        }
      },
    );
    return dispose;
  });
  window.addEventListener('pagehide', withdraw);
  document.addEventListener('freeze', withdraw);
  return () => {
    stopped = true;
    window.removeEventListener('pagehide', withdraw);
    document.removeEventListener('freeze', withdraw);
    stopReloadHold();
    dispose();
    withdraw();
  };
}
