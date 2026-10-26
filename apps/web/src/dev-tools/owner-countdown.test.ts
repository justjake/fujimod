import { afterEach, expect, test, vi } from 'vite-plus/test';
import { createOwnerCountdown } from './owner-countdown';

afterEach(() => vi.useRealTimers());

test('banner and speech count ten seconds before completing', async () => {
  vi.useFakeTimers();
  const display = vi.fn(),
    speak = vi.fn(),
    silence = vi.fn(),
    done = vi.fn();
  const countdown = createOwnerCountdown(display, speak, silence);
  const pending = countdown.start('Film the strip').then(done);
  expect(display).toHaveBeenLastCalledWith({ message: 'Film the strip', remaining: 10 });
  await vi.advanceTimersByTimeAsync(9999);
  expect(done).not.toHaveBeenCalled();
  expect(speak.mock.calls.map(([text]) => text)).toEqual([
    '10',
    '9',
    '8',
    '7',
    '6',
    '5',
    '4',
    '3',
    '2',
    '1',
  ]);
  await vi.advanceTimersByTimeAsync(1);
  await pending;
  expect(done).toHaveBeenCalledOnce();
  expect(display).toHaveBeenLastCalledWith(undefined);
});

test('disconnect cancellation rejects and never completes later', async () => {
  vi.useFakeTimers();
  const display = vi.fn(),
    speak = vi.fn(),
    silence = vi.fn();
  const countdown = createOwnerCountdown(display, speak, silence);
  const pending = countdown.start('Film').catch((error) => error.message);
  await expect(countdown.start('Other action')).rejects.toThrow('already running');
  await vi.advanceTimersByTimeAsync(3000);
  countdown.cancel();
  expect(await pending).toBe('Owner countdown interrupted');
  await vi.advanceTimersByTimeAsync(10000);
  expect(display).toHaveBeenLastCalledWith(undefined);
  expect(speak).toHaveBeenCalledTimes(4);
  expect(silence).toHaveBeenCalledTimes(2);
});

test('speech failure rejects instead of pretending attention completed', async () => {
  const display = vi.fn();
  const countdown = createOwnerCountdown(
    display,
    () => {
      throw Error('Speech unavailable');
    },
    vi.fn(),
  );
  await expect(countdown.start('Film')).rejects.toThrow('Speech unavailable');
  expect(display).toHaveBeenLastCalledWith(undefined);
});
