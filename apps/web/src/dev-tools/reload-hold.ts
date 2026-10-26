import type { CameraModel } from './camera-model';

/** Hold full development reloads while this page owns a connected camera. */
export function holdReloadsDuringCameraSessions(
  model: CameraModel,
  report: (camera: string, message: string) => void,
): () => void {
  const hot = import.meta.hot;
  if (!hot) return () => {};
  const busy = () => model.cameras().filter((camera) => camera.present || camera.sharing);
  let held = false,
    stopped = false;
  const beforeReload = async () => {
    const cameras = busy();
    if (!cameras.length) return;
    if (!held)
      for (const camera of cameras)
        report(camera.id, 'Page update waiting until the camera disconnects');
    held = true;
    while (!stopped && busy().length) await new Promise((resolve) => setTimeout(resolve, 500));
  };
  hot.on('vite:beforeFullReload', beforeReload);
  return () => {
    stopped = true;
    hot.off('vite:beforeFullReload', beforeReload);
  };
}
