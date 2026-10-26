import type { ShareStatus } from './types.ts';
import assert from 'node:assert/strict';
import test from 'node:test';
import { exporterFixture } from './exporter-fixture.ts';
import { ShareHost } from './peer.ts';

// The signaling socket is not exercised: these check the offline state machine only.
function host() {
  const events: (string | ShareStatus)[] = [];
  const device = exporterFixture(
    {
      id: 'cam',
      vendor_id: 0,
      product_id: 0,
      manufacturer: null,
      product: null,
      serial: null,
      interfaces: [],
    },
    undefined,
    () => {
      events.push('freed');
    },
  );
  const share = Object.create(ShareHost.prototype) as ShareHost;
  Object.assign(share, {
    ticket: {},
    device,
    descriptor: JSON.parse(device.inventoryJson)[0],
    offline: undefined,
    battery: undefined,
    callbacks: { state: (status: ShareStatus) => events.push(status) },
    peers: new Map(),
    closed: false,
  });
  return { share, events };
}

await test('a lost camera keeps the share, reports why, and comes back on the same share', () => {
  const { share, events } = host();
  share.battery = { level: 'LOW', at: Date.now() };
  share.cameraLost('battery');
  assert.equal(share.device, undefined);
  assert.deepEqual(
    { ...share.status(), battery_age_ms: 0 },
    { online: false, reason: 'battery', battery: 'LOW', battery_age_ms: 0 },
  );
  assert.deepEqual(
    events.slice(0, 2).map((e) => (typeof e === 'string' ? e : e.online)),
    ['freed', false],
  );
  const returned = exporterFixture({
    id: 'cam2',
    vendor_id: 0,
    product_id: 0,
    manufacturer: null,
    product: null,
    serial: null,
    interfaces: [],
  });
  share.cameraReturned(returned);
  assert.equal(share.device, returned);
  assert.equal(share.descriptor.id, 'cam2');
  assert.equal(share.status().online, true);
});
