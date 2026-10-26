import { test } from 'vitest';
import assert from 'node:assert/strict';
import { browserFromUserAgent, installErrorLog, type ErrorLogRecord } from './error-log';

test('uncaught errors, console errors and emulator faults are logged once each without query or fragment', () => {
  const records: ErrorLogRecord[] = [];
  history.replaceState(null, '', '/dev-tools/?room=secret#token');
  const error = console.error;
  console.error = () => {};
  try {
    installErrorLog((record) => records.push(record));
    const failure = new TypeError("null is not an object (evaluating 'node.nextSibling')");
    dispatchEvent(new ErrorEvent('error', { error: failure, message: failure.message }));
    dispatchEvent(new ErrorEvent('error', { error: failure, message: failure.message }));
    console.error('Hydration mismatch', { node: 'p' });
    dispatchEvent(
      new CustomEvent('fujimod-fault', {
        detail: {
          kind: 'guest-fault',
          message: 'The camera stopped after an execution fault.',
          details: { type: 'GuestExecutionError', error: 'Invalid memory write' },
        },
      }),
    );
  } finally {
    console.error = error;
  }
  assert.deepEqual(
    records.map(({ kind, message }) => [kind, message]),
    [
      ['error', "TypeError: null is not an object (evaluating 'node.nextSibling')"],
      ['console', 'Hydration mismatch {"node":"p"}'],
      [
        'fault',
        'The camera stopped after an execution fault. GuestExecutionError: Invalid memory write',
      ],
    ],
  );
  assert.ok(records.every((record) => record.session === records[0].session));
  assert.equal(records[0].page, `${location.origin}/dev-tools/`);
  assert.ok(records[0].stack?.includes('error-log.test'));
  assert.equal(records[2].fault?.kind, 'guest-fault');
  const { device } = records[0];
  assert.ok(!('unavailable' in device));
  assert.equal(device.viewport.width, innerWidth);
  assert.equal(device.screen.pixelRatio, devicePixelRatio);
  assert.equal(typeof device.timeZone, 'string');
  assert.equal(typeof device.crossOriginIsolated, 'boolean');
});

test('user agents name the browser and OS', () => {
  const safari =
    'Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0.1 Mobile/15E148 Safari/604.1';
  assert.deepEqual(browserFromUserAgent(safari), { browser: 'Safari 27.0.1', os: 'iOS' });
  assert.deepEqual(browserFromUserAgent(`${safari} Brave`), {
    browser: 'Brave (WebKit 27.0.1)',
    os: 'iOS',
  });
  assert.deepEqual(
    browserFromUserAgent(
      'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36',
    ),
    { browser: 'Chrome 156', os: 'macOS' },
  );
  assert.deepEqual(
    browserFromUserAgent(
      'Mozilla/5.0 (Linux; Android 16; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Mobile Safari/537.36',
    ),
    { browser: 'Chrome 156', os: 'Android 16' },
  );
});
