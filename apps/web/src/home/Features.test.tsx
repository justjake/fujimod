import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, beforeEach, expect, test, vi } from 'vite-plus/test';
import { FeatureCatalog } from './Features';
import catalog from './features.generated.json';
import metrics from '../ui/assets/feature-icon-metrics.generated.json';
import type { Media } from './feature-media';

const fixture = vi.hoisted(() => ({ cards: {} as Record<string, Media> }));
vi.mock('./feature-media', () => ({
  featureMedia:
    () =>
    (...cards: string[]) =>
      cards.map((card) => fixture.cards[card]).find(Boolean) ?? null,
}));

let dispose: (() => void) | undefined;
const scrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
beforeEach(() => {
  fixture.cards = {};
  Object.defineProperty(Element.prototype, 'scrollIntoView', {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.replaceChildren();
  history.replaceState(null, '', '/');
  if (scrollIntoView) Object.defineProperty(Element.prototype, 'scrollIntoView', scrollIntoView);
  else Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
});

test('the complete text catalog renders without media and search includes native option labels', () => {
  dispose = render(() => <FeatureCatalog />, document.body);
  flush();
  expect(document.querySelectorAll('.catalog-tile')).toHaveLength(catalog.features.length);
  expect(document.querySelectorAll('img, video')).toHaveLength(0);
  const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
  search.value = 'eterna';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flush();
  expect(document.querySelectorAll('.catalog-tile').length).toBeGreaterThan(0);
  expect(document.querySelectorAll('.catalog-tile').length).toBeLessThan(catalog.features.length);
  expect(document.querySelector('.catalog-grid')?.textContent).toContain('FILM SIMULATION');
  search.value = 'no-such-feature';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flush();
  expect(document.querySelectorAll('.catalog-tile')).toHaveLength(0);
  expect(document.querySelector('[role="status"]')?.textContent).toBe('No matching features.');
  search.value = '';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flush();
  expect(document.querySelectorAll('.catalog-tile')).toHaveLength(catalog.features.length);
});

test('feature deep links filter the catalog on entry and on repeated navigation', () => {
  history.replaceState(null, '', '/#features?search=film');
  const original = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
  const scroll = vi.fn();
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, value: scroll });
  try {
    dispose = render(() => <FeatureCatalog />, document.body);
    flush();
    const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
    expect(search.value).toBe('film');
    history.replaceState(null, '', '/#features?search=focus');
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    flush();
    expect(search.value).toBe('focus');
    expect(document.querySelector('.catalog-grid')?.textContent).toMatch(/FOCUS/);
    expect(scroll).toHaveBeenCalled();
  } finally {
    if (original) Object.defineProperty(Element.prototype, 'scrollIntoView', original);
    else Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
  }
});

test('every generated feature and option icon has measured native artwork', () => {
  const icons = new Set([
    ...catalog.features.map((feature) => feature.icon),
    ...catalog.features.flatMap((feature) =>
      feature.controls.flatMap((control) => control.choices.map((choice) => choice.icon)),
    ),
  ]);
  for (const icon of icons)
    if (icon && !icon.startsWith('ratio:')) expect(Object.keys(metrics)).toContain(icon);
});

function mountMedia(hash = '') {
  fixture.cards.image_size = {
    video: '111111111111111111111111.mp4',
    still: '222222222222222222222222.webp',
    clips: {},
  };
  history.replaceState(null, '', `/${hash}`);
  dispose = render(() => <FeatureCatalog />, document.body);
  flush();
}

function reference(title: string, within: ParentNode = document) {
  const element = [...within.querySelectorAll<HTMLElement>('.catalog-ref')].find(
    (element) => element.textContent === title,
  );
  expect(element).toBeDefined();
  return element!;
}

function expectImageSize() {
  expect(document.querySelector('.catalog-card.on .catalog-card-title')?.textContent).toBe(
    'IMAGE SIZE',
  );
  expect(document.querySelector('.catalog-card-video')?.getAttribute('src')).toBe(
    '/artifacts/docs/111111111111111111111111.mp4',
  );
  expect(document.querySelector('.catalog-card-video')?.getAttribute('poster')).toBe(
    '/artifacts/docs/222222222222222222222222.webp',
  );
}

test('Related opens its own named card and media with the catalog pointer interaction', () => {
  mountMedia();
  const aspect = reference('ASPECT RATIO');
  aspect.getBoundingClientRect = () => new DOMRect(200, 100, 80, 20);
  aspect.click();
  flush();
  expect(document.querySelector('.catalog-card-title')?.textContent).toBe('ASPECT RATIO');
  expect(document.querySelector('.catalog-card-video')).toBeNull();
  const related = reference('IMAGE SIZE', document.querySelector('.catalog-card-related')!);
  expect(related.getAttribute('role')).toBe('button');
  expect(related.tabIndex).toBe(0);
  related.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));
  related.focus();
  related.click();
  flush();
  expectImageSize();
  expect(document.querySelector<HTMLElement>('.catalog-card')?.style.top).toBe('130px');
  expect(new URLSearchParams(location.hash.split('?')[1]).get('card')).toBe('image_size');
});

test.each(['Enter', ' '])('Related IMAGE SIZE opens with the %j key', (key) => {
  mountMedia();
  reference('ASPECT RATIO').focus();
  flush();
  const related = reference('IMAGE SIZE', document.querySelector('.catalog-card-related')!);
  related.focus();
  flush();
  expect(document.activeElement).toBe(related);
  expect(document.querySelector('.catalog-card-title')?.textContent).toBe('ASPECT RATIO');
  related.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  flush();
  expectImageSize();
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  flush();
  expect(document.querySelector('.catalog-card.on')).toBeNull();
  expect(new URLSearchParams(location.hash.split('?')[1]).has('card')).toBe(false);
});

test('card deep links open after entry settles and preserve normal search navigation', () => {
  mountMedia('#features?search=aspect&card=image_size');
  expectImageSize();
  const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
  expect(search.value).toBe('aspect');
  expect(document.querySelector('.catalog-grid')?.textContent).toContain('ASPECT RATIO');
  history.replaceState(null, '', '/#features?search=focus');
  window.dispatchEvent(new HashChangeEvent('hashchange'));
  flush();
  expect(document.querySelector('.catalog-card.on')).toBeNull();
  expect(search.value).toBe('focus');
  expect(document.querySelector('.catalog-grid')?.textContent).toMatch(/FOCUS/);
  history.replaceState(null, '', '/#features?card=image_size');
  window.dispatchEvent(new HashChangeEvent('hashchange'));
  flush();
  expectImageSize();
  expect(search.value).toBe('');
});

test.each(['no-such-card', '__proto__', 'constructor'])(
  'unknown deep-link card %s is ignored without breaking search',
  (card) => {
    mountMedia(`#features?search=film&card=${card}`);
    expect(document.querySelector('.catalog-card.on')).toBeNull();
    expect(document.querySelector<HTMLInputElement>('input[type="search"]')?.value).toBe('film');
    expect(document.querySelector('.catalog-grid')?.textContent).toContain('Film Grain');
  },
);
