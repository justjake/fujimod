import { render } from '@solidjs/web';
import { readFileSync } from 'node:fs';
import { flush } from 'solid-js';
import { afterEach, expect, test, vi } from 'vite-plus/test';
import { ReleaseShowcase } from './ReleaseShowcase';
import { PersonalNote } from '../ui/personal-note';
import homeSource from './Page.tsx?raw';
import { FeatureCatalog } from './Features';
import catalog from './features.generated.json';
import metrics from '../ui/assets/feature-icon-metrics.generated.json';
import type { LoadedRelease } from '../patch/release';

vi.mock('./feature-media', () => ({ featureMedia: () => () => null }));

let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
  history.replaceState(null, '', '/');
});

const release: LoadedRelease = {
  schema: 2,
  release: '0.1.0',
  tag: '0.1.0',
  name: '0.1.0',
  published_at: '2026-11-13T00:00:00Z',
  prerelease: true,
  patches: {},
  firmwares: [],
  assetBase: 'https://example.org/releases/0.1.0/',
  notesText: '## Known issues\n- Preview limitation.',
};

test('the original opening note precedes the retained release highlights', () => {
  expect(homeSource).toMatch(/<PersonalNote\s*\/>\s*<ReleaseShowcase/);
  dispose = render(
    () => (
      <>
        <PersonalNote />
        <ReleaseShowcase />
      </>
    ),
    document.body,
  );
  flush();
  expect(document.querySelector('.personal-note')?.textContent).toContain('Hey,');
  expect(document.querySelector('.signature-name')?.textContent).toBe('Jake');
  expect(document.querySelector('h1')).toBeNull();
  expect(document.body.textContent).not.toContain('A familiar camera.');
  expect(document.querySelector('.release-eyebrow')?.textContent).toContain('Preview');
  expect(
    [...document.querySelectorAll('.release-bodies li')].map((node) => node.textContent),
  ).toEqual(['X70Firmware 1.10', 'X-E2SFirmware 1.11', 'X-E2Firmware 4.11']);
  expect(document.querySelectorAll('.release-highlight')).toHaveLength(6);
  expect(document.body.textContent).toContain('255 custom settings on X70');
  expect(document.body.textContent).toContain('catalog below describes X70');
  expect(document.querySelector('#release-notes')).toBeNull();
  expect(document.querySelector('a[href="#quickstart"]')).not.toBeNull();
});

test('release accents use the existing neutral site palette', () => {
  const showcaseStyle = readFileSync('src/home/release-showcase.css', 'utf8');
  expect(showcaseStyle).not.toMatch(/#(?:385945|b1d6b9|42684d|263d2d|e7f5df|a5b9aa)\b/i);
  expect(showcaseStyle).toContain('background: var(--key)');
  expect(showcaseStyle).toContain('border: 1px solid var(--edge)');
  expect(showcaseStyle).toMatch(/\.release-highlight > \.catalog-icon\s*\{[^}]*color: #eee;/);
});

test('release notes come from the loaded release, without calling a candidate published', () => {
  dispose = render(() => <ReleaseShowcase release={release} />, document.body);
  flush();
  expect(document.querySelector('#release-notes summary')?.textContent).toContain('0.1.0 preview');
  expect(document.querySelector('#release-notes')?.textContent).toContain('Preview limitation.');
});

test('a published 0.1.0 no longer carries the preview badge', () => {
  dispose = render(
    () => <ReleaseShowcase release={{ ...release, prerelease: false }} />,
    document.body,
  );
  flush();
  expect(document.querySelector('.release-eyebrow')?.textContent).not.toContain('Preview');
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

test('highlight deep links filter the catalog on entry and on repeated navigation', () => {
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
