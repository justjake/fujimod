import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import { FeatureCatalog } from './Features';
import catalog from './features.generated.json';
import metrics from '../ui/assets/feature-icon-metrics.generated.json';

vi.mock('./feature-media', () => ({ featureMedia: () => () => null }));

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
  history.replaceState(null, '', '/');
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
