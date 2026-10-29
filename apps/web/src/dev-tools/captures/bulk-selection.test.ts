import { expect, test, vi } from 'vite-plus/test';

vi.mock('fujimod', () => ({
  catalog: {
    selections: () => [],
    bulkProfiles: () => [{ firmware_id: 'xe4-201', product: 'X-E4', internal_version: '1.43' }],
    firmwareVersions: () => [{ firmware_id: 'xe4-201', version: '2.01' }],
  },
}));

const { bulkSelection } = await import('./capture');

test('selects a located reader without an execution profile', () => {
  const selected = bulkSelection({ product: 'X-E4', internalVersion: '1.43' });
  expect(selected?.bulk.firmware_id).toBe('xe4-201');
  expect(selected?.firmware.version).toBe('2.01');
  expect(bulkSelection({ product: 'X-E4', internalVersion: '1.44' })).toBeUndefined();
});
