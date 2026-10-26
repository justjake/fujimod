import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, beforeEach, expect, test, vi } from 'vite-plus/test';
import { FeatureTile } from './FeatureTile';

let dispose: (() => void) | undefined;
const play = vi.fn<() => Promise<void>>();
const pause = vi.fn<() => void>();
const load = vi.fn<() => void>();
const observers: TestIntersectionObserver[] = [];
const observer = () => observers[0]!;
let media: EventTarget & { matches: boolean };

class TestIntersectionObserver implements IntersectionObserver {
  root = null;
  rootMargin = '0px';
  scrollMargin = '0px';
  thresholds = [0, 0.25, 0.5, 0.75, 1];
  observe = vi.fn<(element: Element) => void>();
  unobserve = vi.fn<(element: Element) => void>();
  disconnect = vi.fn();
  constructor(private callback: IntersectionObserverCallback) {
    observers.push(this);
  }
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
  emit(elements: Element[], ratios: number[]) {
    this.callback(
      elements.map((target, index) => ({
        target,
        intersectionRatio: ratios[index],
        isIntersecting: ratios[index] > 0,
        time: 0,
        boundingClientRect: target.getBoundingClientRect(),
        intersectionRect: target.getBoundingClientRect(),
        rootBounds: null,
      })),
      this,
    );
    flush();
  }
}

// Fades and the touch settle delay run on timers; settle() lets them finish.
const settle = () => {
  vi.advanceTimersByTime(1000);
  flush();
};

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  observers.length = 0;
  play.mockReset().mockResolvedValue();
  pause.mockReset();
  load.mockReset();
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockImplementation(play);
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(pause);
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(load);
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  media = Object.assign(new EventTarget(), { matches: false });
  vi.stubGlobal('matchMedia', () => media);
  vi.stubGlobal('IntersectionObserver', TestIntersectionObserver);
});
afterEach(() => {
  vi.useRealTimers();
  dispose?.();
  dispose = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function mount(mobile = false) {
  media.matches = mobile;
  const host = document.createElement('div');
  document.body.append(host);
  dispose = render(
    () => (
      <>
        {[1, 2].map((id) => (
          <FeatureTile media={{ still: `hero-${id}.webp`, video: `hero-${id}.mp4` }}>
            <h4>Feature {id}</h4>
            <button>Option</button>
          </FeatureTile>
        ))}
      </>
    ),
    host,
  );
  flush();
  const tiles = [...host.querySelectorAll('article')];
  const previews = [...host.querySelectorAll('.catalog-tile-preview')];
  const fire = (type: string, index = 0) => {
    tiles[index].dispatchEvent(new MouseEvent(type));
    settle();
  };
  const visible = (...ratios: number[]) => {
    observer().emit(previews, ratios);
    settle();
  };
  return { host, tiles, fire, visible };
}

test('desktop mounts only a hovered recording and unloads it on leave', () => {
  const { host, fire, visible } = mount();
  visible(1, 1);
  expect(host.querySelectorAll('video')).toHaveLength(0);
  expect(play).not.toHaveBeenCalled();
  fire('mouseenter');
  const video = host.querySelector('video')!;
  expect(video.getAttribute('src')).toBe('/artifacts/docs/hero-1.mp4');
  expect(play).toHaveBeenCalledOnce();
  expect(video.classList.contains('playing')).toBe(false);
  video.dispatchEvent(new Event('playing'));
  flush();
  expect(video.classList.contains('playing')).toBe(true);
  fire('mouseleave');
  expect(host.querySelector('video')).toBeNull();
  expect(pause).toHaveBeenCalledOnce();
  expect(load).toHaveBeenCalledOnce();
  expect(video.hasAttribute('src')).toBe(false);
  fire('mouseenter');
  expect(host.querySelector('video')).not.toBe(video);
});

test('mobile plays one visible preview and unloads recordings after scrolling', () => {
  const { host, tiles, fire, visible } = mount(true);
  expect(host.querySelector('video')).toBeNull();
  visible(1, 0.5);
  const first = host.querySelector('video')!;
  expect(tiles[0].querySelector('video')).toBe(first);
  expect(host.querySelectorAll('video')).toHaveLength(1);
  // Touch-generated focus and hover must not pin a card after scrolling away.
  tiles[0].focus();
  fire('mouseenter');
  visible(0, 1);
  expect(first.hasAttribute('src')).toBe(false);
  expect(first.isConnected).toBe(false);
  expect(load).toHaveBeenCalledOnce();
  expect(tiles[1].querySelector('video')).not.toBeNull();
  expect(host.querySelectorAll('video')).toHaveLength(1);
  visible(0, 0.25);
  expect(host.querySelector('video')).toBeNull();
  expect(load).toHaveBeenCalledTimes(2);
  visible(0.75, 0);
  expect(tiles[0].querySelector('video')).not.toBeNull();
  expect(host.querySelectorAll('img')).toHaveLength(2);
});

test('hidden tabs release playback; mode changes and disposal clean up observation', () => {
  const { host, fire, visible } = mount(true);
  visible(1, 0);
  expect(host.querySelectorAll('video')).toHaveLength(1);
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  document.dispatchEvent(new Event('visibilitychange'));
  settle();
  expect(host.querySelector('video')).toBeNull();
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  document.dispatchEvent(new Event('visibilitychange'));
  settle();
  expect(host.querySelectorAll('video')).toHaveLength(1);
  media.matches = false;
  media.dispatchEvent(new Event('change'));
  settle();
  expect(host.querySelector('video')).toBeNull();
  fire('mouseenter');
  expect(host.querySelectorAll('video')).toHaveLength(1);
  dispose?.();
  dispose = undefined;
  expect(observer().disconnect).toHaveBeenCalledOnce();
  expect(observer().unobserve).toHaveBeenCalledTimes(2);
  expect(host.querySelector('video')).toBeNull();
});

test('desktop focus survives internal navigation but cannot retain an offscreen video', () => {
  const { host, tiles, fire, visible } = mount();
  visible(1, 1);
  tiles[0].focus();
  flush();
  tiles[0].querySelector('button')!.focus();
  flush();
  expect(pause).not.toHaveBeenCalled();
  fire('mouseenter');
  tiles[0].querySelector('button')!.blur();
  flush();
  expect(pause).not.toHaveBeenCalled();
  visible(0, 1);
  expect(host.querySelector('video')).toBeNull();
  expect(load).toHaveBeenCalledOnce();
});

test('a late rejection from an unloaded video cannot disable its replacement', async () => {
  let reject: ((error: Error) => void) | undefined;
  play.mockImplementationOnce(
    () =>
      new Promise((_, fail) => {
        reject = fail;
      }),
  );
  const { host, fire, visible } = mount();
  visible(1, 0);
  fire('mouseenter');
  const first = host.querySelector('video');
  fire('mouseleave');
  fire('mouseenter');
  const replacement = host.querySelector('video');
  reject!(new Error('Playback aborted by unload'));
  await Promise.resolve();
  flush();
  expect(replacement).not.toBe(first);
  expect(host.querySelector('video')).toBe(replacement);
  replacement!.dispatchEvent(new Event('error'));
  settle();
  expect(host.querySelector('video')).toBeNull();
  expect(host.querySelectorAll('img')).toHaveLength(2);
});

test('desktop hover takes priority over focus and returns focus playback afterward', () => {
  const { host, tiles, fire, visible } = mount();
  visible(1, 1);
  tiles[0].focus();
  flush();
  expect(tiles[0].querySelector('video')).not.toBeNull();
  fire('mouseenter', 1);
  expect(tiles[1].querySelector('video')).not.toBeNull();
  expect(tiles[0].querySelector('video')).toBeNull();
  fire('mouseleave', 1);
  expect(tiles[0].querySelector('video')).not.toBeNull();
  expect(host.querySelectorAll('video')).toHaveLength(1);
});

test('a preview fades out over its still before unloading, and returning keeps it', () => {
  const { host, tiles, visible } = mount();
  visible(1, 0);
  tiles[0].dispatchEvent(new MouseEvent('mouseenter'));
  flush();
  const video = host.querySelector('video')!;
  video.dispatchEvent(new Event('playing'));
  flush();
  tiles[0].dispatchEvent(new MouseEvent('mouseleave'));
  flush();
  expect(video.classList.contains('leaving')).toBe(true);
  expect(video.isConnected).toBe(true);
  tiles[0].dispatchEvent(new MouseEvent('mouseenter'));
  settle();
  expect(host.querySelector('video')).toBe(video);
  expect(video.classList.contains('leaving')).toBe(false);
  expect(load).not.toHaveBeenCalled();
});

test('touch scrolling keeps a mostly visible preview and starts a new one once settled', () => {
  const { tiles, visible } = mount(true);
  visible(1, 0.5);
  const first = tiles[0].querySelector('video')!;
  visible(0.75, 1);
  expect(tiles[0].querySelector('video')).toBe(first);
  expect(tiles[1].querySelector('video')).toBeNull();
  observer().emit([...document.querySelectorAll('.catalog-tile-preview')], [0.25, 1]);
  expect(tiles[1].querySelector('video')).toBeNull();
  settle();
  expect(tiles[1].querySelector('video')).not.toBeNull();
  expect(tiles[0].querySelector('video')).toBeNull();
});
