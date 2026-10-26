import { createRoot } from 'solid-js';
import { expect, test, vi } from 'vite-plus/test';
const { startDevShare } = vi.hoisted(() => ({
  startDevShare: vi.fn(
    (_model: import('./camera-model').CameraModel, _options: unknown) => () => {},
  ),
}));
vi.mock('./dev-share', () => ({ startDevShare }));
vi.mock('fujimod', () => ({
  initialize: async () => {},
  usb: { discover: async () => [], DeviceFilter: { Diagnostic: 0 } },
}));
test('one model survives component disposal and module hot swaps', async () => {
  const pinned = window as Window & { __fujimodCameraModel?: import('./camera-model').CameraModel };
  delete pinned.__fujimodCameraModel;
  const { pageCameraModel } = await import('./usb-camera-model');
  let release!: () => void;
  const first = createRoot((dispose) => {
    release = dispose;
    return pageCameraModel();
  });
  release();
  expect(pageCameraModel()).toBe(first);
  vi.resetModules();
  const hot = await import('./usb-camera-model');
  expect(hot.pageCameraModel()).toBe(first);
  expect(startDevShare).toHaveBeenCalledOnce();
  expect(startDevShare.mock.calls[0][0]).toBe(first);
  first.dispose();
  delete pinned.__fujimodCameraModel;
});
