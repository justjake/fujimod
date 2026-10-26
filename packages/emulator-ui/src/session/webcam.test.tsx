import { afterEach, expect, test, vi } from 'vite-plus/test';
import { WebcamInput } from './webcam';
import type { Scene } from '../engine/api';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function camera() {
  const stop = vi.fn();
  const track = Object.assign(new EventTarget(), { stop }) as unknown as MediaStreamTrack;
  const stream = {
    getTracks: () => [track],
    getVideoTracks: () => [track],
  } as unknown as MediaStream;
  const getUserMedia = vi.fn(async (_constraints: MediaStreamConstraints) => stream);
  vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, 'readyState', 'get').mockReturnValue(4);
  vi.spyOn(HTMLVideoElement.prototype, 'videoWidth', 'get').mockReturnValue(2);
  vi.spyOn(HTMLVideoElement.prototype, 'videoHeight', 'get').mockReturnValue(1);
  const context = {
    drawImage: vi.fn(),
    getImageData: () => ({ data: new Uint8ClampedArray([255, 0, 0, 255, 0, 255, 0, 255]) }),
  };
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(
    context as unknown as CanvasRenderingContext2D,
  );
  return { stop, stream, getUserMedia, track };
}

test('webcam frames use RGB and wait for the emulator; stopping releases capture and restores the still', async () => {
  vi.useFakeTimers();
  const source = camera();
  let acknowledge!: () => void;
  const pending = new Promise<void>((resolve) => {
    acknowledge = resolve;
  });
  const send = vi.fn(async (scene: Scene | null) => {
    if (scene) await pending;
  });
  const changed = vi.fn(),
    failed = vi.fn();
  const input = new WebcamInput(send, changed, failed);
  await input.start();
  expect(source.getUserMedia.mock.calls[0][0].audio).toBe(false);
  expect(send.mock.calls[0][0]).toEqual({
    width: 2,
    height: 1,
    rgb: new Uint8Array([255, 0, 0, 0, 255, 0]),
  });
  await vi.advanceTimersByTimeAsync(1000);
  expect(send).toHaveBeenCalledTimes(1);
  acknowledge();
  await vi.advanceTimersByTimeAsync(100);
  expect(send).toHaveBeenCalledTimes(2);
  await input.stop();
  await vi.advanceTimersByTimeAsync(1000);
  expect(send).toHaveBeenCalledTimes(3);
  expect(send).toHaveBeenLastCalledWith(null);
  expect(source.stop).toHaveBeenCalledOnce();
  expect(changed).toHaveBeenLastCalledWith(false, false);
  expect(failed).not.toHaveBeenCalled();
});

test('canceling a pending permission request stops a stream that arrives later', async () => {
  const source = camera();
  let allow!: (stream: MediaStream) => void;
  source.getUserMedia.mockReturnValue(
    new Promise((resolve) => {
      allow = resolve;
    }),
  );
  const send = vi.fn(async () => {}),
    changed = vi.fn();
  const input = new WebcamInput(send, changed, vi.fn());
  const starting = input.start();
  await input.stop(false);
  allow(source.stream);
  await starting;
  expect(source.stop).toHaveBeenCalledOnce();
  expect(send).not.toHaveBeenCalled();
  expect(changed).not.toHaveBeenCalledWith(true, false);
});

test('permission denial leaves capture off and reports the error', async () => {
  const source = camera();
  const error = new DOMException('Denied', 'NotAllowedError');
  source.getUserMedia.mockRejectedValue(error);
  const changed = vi.fn(),
    failed = vi.fn();
  const input = new WebcamInput(async () => {}, changed, failed);
  await input.start();
  expect(input.requested).toBe(false);
  expect(changed).toHaveBeenLastCalledWith(false, false);
  expect(failed).toHaveBeenCalledWith(error);
});
