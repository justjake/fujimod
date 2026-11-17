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
    expect(host.querySelector('a[href="/credits"]')?.textContent).toBe('everyone!');
    expect(
      [...host.querySelectorAll('.note-names > span')].map((name) => name.textContent),
    ).toEqual(['Ada Example', '@zoe']);
  } finally {
    dispose();
  }
});
