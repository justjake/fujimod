import assert from 'node:assert/strict';
import test from 'node:test';
import { ShareHost } from './peer.ts';
import { exporterFixture } from './exporter-fixture.ts';

test('stopping a share frees its exporter handle without shutting down the Rust owner', () => {
  const calls: string[] = [];
  const device = exporterFixture(
    {
      id: 'camera',
      vendor_id: 0x4cb,
      product_id: 0xff80,
      manufacturer: null,
      product: null,
      serial: null,
      interfaces: [],
    },
    undefined,
    () => calls.push('free'),
  );
  device.shutdown = async () => {
    calls.push('shutdown');
  };
  const host = Object.create(ShareHost.prototype) as ShareHost;
  Object.assign(host, {
    device,
    peers: new Map(),
    callbacks: {},
    signal: { close: () => {} },
    closed: false,
  });
  host.stop();
  assert.deepEqual(calls, ['free']);
});
