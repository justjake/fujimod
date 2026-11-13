import { expect, test } from 'vite-plus/test';
import { detectSystem, FORMAT_STEPS } from './format-guide';

test('detectSystem reads the platform from the user agent', () => {
  expect(detectSystem('Mozilla/5.0 (Windows NT 10.0)')).toBe('Windows');
  expect(detectSystem('Mozilla/5.0 (X11; Linux x86_64)')).toBe('Linux');
  expect(detectSystem('Mozilla/5.0 (Linux; Android 14)')).toBe('macOS');
  expect(detectSystem('Mozilla/5.0 (Macintosh; Intel Mac OS X)')).toBe('macOS');
});

test('every system lists FAT32 formatting steps', () => {
  for (const steps of Object.values(FORMAT_STEPS)) expect(steps.length).toBeGreaterThan(0);
});
