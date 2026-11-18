import { render } from '@solidjs/web';
import { expect, test } from 'vite-plus/test';
import { ReleasePersonalNote } from './ReleasePersonalNote';

test('renders the release date, elapsed days, and names in the credits hover list', () => {
  const host = document.createElement('div');
  const dispose = render(
    () => (
      <ReleasePersonalNote
        data={{
          current: { version: 'v2.0.0', publishedAt: '2026-04-02T00:05:00Z', notes: '' },
          past: [],
          personalNote: {
            daysSinceFirstRelease: 23,
            contributorCount: 2,
            contributorNames: ['Ada Example', '@zoe'],
          },
        }}
      />
    ),
    host,
  );
  try {
    expect(host.querySelector('article time')?.textContent).toBe('April 2, 2026');
    expect(host.querySelector('time')?.dateTime).toBe('2026-04-02T00:05:00Z');
    expect(host.textContent).toContain('23 days');
    expect(host.textContent).toContain('2 contributors');
    expect(
      [...host.querySelectorAll('a[href="/credits"]')].map((link) => link.textContent),
    ).toEqual(['everyone!', 'All credits →']);
    expect(host.querySelector('.note-names[role="region"]')?.getAttribute('tabindex')).toBe('0');
    expect(
      [...host.querySelectorAll('.note-names > span')].map((name) => name.textContent),
    ).toEqual(['Ada Example', '@zoe']);
  } finally {
    dispose();
  }
});

test('shows the under construction note until a release newer than 0.0.1 is published', () => {
  const host = document.createElement('div');
  const dispose = render(
    () => (
      <ReleasePersonalNote
        data={{
          current: { version: '0.0.1', publishedAt: '2026-10-27T18:58:45Z', notes: '' },
          past: [],
          personalNote: { daysSinceFirstRelease: 0, contributorCount: 0, contributorNames: [] },
        }}
      />
    ),
    host,
  );
  try {
    expect(host.querySelector('article time')?.textContent).toBe('November 18, 2026');
    expect(host.textContent).toContain('Under construction!');
    expect(host.textContent).toContain('Thanks for your patience,');
    expect(host.textContent).not.toContain('second version');
  } finally {
    dispose();
  }
});
