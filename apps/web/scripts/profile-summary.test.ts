import assert from 'node:assert/strict';
import { test } from 'node:test';
import { summarizeProfile, type CpuProfile } from './profile-summary.ts';

await test('weights uneven samples and counts recursive functions once in inclusive totals', () => {
  const profile: CpuProfile = {
    nodes: [
      { id: 1, callFrame: { functionName: '(root)', url: '' }, children: [2, 4] },
      {
        id: 2,
        callFrame: { functionName: 'helper', url: 'https://example.test/camera.wasm' },
        children: [3],
      },
      { id: 3, callFrame: { functionName: 'helper', url: 'https://example.test/camera.wasm' } },
      { id: 4, callFrame: { functionName: 'b0', url: 'wasm://wasm/generated' } },
    ],
    samples: [2, 3, 4],
    timeDeltas: [1000, 3000, 6000],
  };
  const result = summarizeProfile(profile);
  assert.equal(result.sampledSeconds, 0.01);
  assert.deepEqual(
    result.categories.map((row) => [row.name, row.percent]),
    [
      ['generated Wasm', 60],
      ['emulator Wasm', 40],
    ],
  );
  assert.equal(result.inclusive.find((row) => row.name === 'helper')!.percent, 40);
  assert.equal(result.inclusive.find((row) => row.name === '(root)')!.percent, 100);
  assert.throws(() => summarizeProfile({ ...profile, timeDeltas: [] }), /lengths differ/);
});
