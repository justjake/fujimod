import { expect, test } from 'vite-plus/test';
import { SHARING_ORIGIN, sharingUrl } from './sharing-server';
import { resolveShareUrl } from './files/share-url';

const id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
test('tests and production builds use the public sharing server', () => {
  expect(SHARING_ORIGIN).toBe('https://share.fujimod.com');
});
test('the client builds every sharing request from one build-time origin', () => {
  expect(sharingUrl('/proxy?url=x')).toBe(SHARING_ORIGIN + '/proxy?url=x');
  expect(sharingUrl('/error-logs')).toBe(SHARING_ORIGIN + '/error-logs');
});
test('capabilities resolve only on the origin the Worker was reached through', () => {
  const preview = 'https://preview.example';
  expect(resolveShareUrl('/contribution/' + id, preview)).toBe(preview + '/contribution/' + id);
  expect(resolveShareUrl(sharingUrl('/contribution/' + id), SHARING_ORIGIN)).toBe(
    sharingUrl('/contribution/' + id),
  );
  for (const value of [
    'javascript:alert(1)',
    'https://other.example/contribution/' + id,
    preview + '/contribution/' + id + '/files',
    '/contribution/' + id.slice(1),
    undefined,
  ])
    expect(resolveShareUrl(value, preview)).toBeUndefined();
});
