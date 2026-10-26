// Regression for the owner's X100V report, 2026-10-20: identity succeeded and
// C4 completed (12 B OUT / 2 B IN), then USBDevice.close() overlapped an
// interface-state change. This exercises production Wasm/nusb, with no USB hardware.
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, usb } from '../dist/index.js';
import { installWebUsb } from './webusb-fixture.ts';
import { encodeMessage, decodeMessage } from '../../../crates/usb-proxy/web/wire.ts';
import type { Completion, Operation } from '../../../crates/usb-proxy/web/types.ts';
await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});
const fixture = installWebUsb();
fixture.state.interfaceDelay = 20;
fixture.iface.alternates[0].endpoints = [
  { endpointNumber: 2, direction: 'in', type: 'bulk', packetSize: 64 },
  { endpointNumber: 1, direction: 'out', type: 'bulk', packetSize: 64 },
];
fixture.iface.alternate = fixture.iface.alternates[0];
try {
  await test('fixture refuses overlapping open/close during interface-state changes', async () => {
    const changing = fixture.device.releaseInterface(2);
    await assert.rejects(
      fixture.device.close(),
      /operation that changes interface state is in progress/,
    );
    await assert.rejects(
      fixture.device.open(),
      /operation that changes interface state is in progress/,
    );
    await changing;
    fixture.log.length = 0;
  });
  await test('one Rust owner serves local and remote sessions through share start and stop', async () => {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic);
    const events = device.watch();
    const logs = device.watchLog();
    assert.equal((await events.next())!.present, true);
    const [rediscovered] = await usb.discover(usb.DeviceFilter.Diagnostic);
    assert.equal(rediscovered.id, device.id);
    await device.open();
    const selection = new usb.InterfaceSelection(1, 2, 3),
      limits = new usb.FirmwareLimits(256, 256, 256, 256),
      options = new usb.TransferOptions(1000, 4096);
    const acquire = () => device.acquireSession(selection, limits, options);
    const read = async () => {
      const session = await acquire();
      assert.equal((await session.deviceInfo(usb.InfoSelector.Product)).text, 'SYNTHETIC');
      assert.deepEqual(await session.close(), Uint8Array.of(0xaa, 0xbb));
    };
    await read();
    const firstExchanges = [];
    for (let n = 0; n < 4; n++) firstExchanges.push((await logs.next())!);
    assert.deepEqual(
      firstExchanges.map((event) => event.kind),
      [0xc0, 0xd0, 0xd1, 0xc4],
    );
    assert.equal(new Set(firstExchanges.map((event) => event.sequence)).size, 4);
    for (const event of firstExchanges) {
      assert.equal(event.deviceId, device.id);
      assert.deepEqual(event.holder, { kind: 'local' });
      assert.ok(event.envelope instanceof Uint8Array);
      assert.ok(event.data instanceof Uint8Array);
      assert.equal(event.error, null);
    }
    assert.equal(firstExchanges[1].operation, 'Device info');
    assert.equal(firstExchanges[1].selector, 4);
    assert.equal(firstExchanges[2].selector, firstExchanges[1].selector);
    assert.deepEqual(firstExchanges[3].data, Uint8Array.of(0xaa, 0xbb));
    const exporter = device.share();
    assert.equal(device.sharing, true);
    await read();
    device.stopSharing();
    exporter.free();
    await read();
    assert.equal(device.status.open, true);
    assert.equal(device.status.fenced, false);
    await device.close();
    assert.equal(fixture.state.interfaceChanging, false);
    await device.open();
    await read();
    assert.equal(device.id, rediscovered.id);
    const remote = device.share();
    let seq = 0;
    const request = async (controller: string, op: Operation): Promise<Completion> => {
      const sent = ++seq;
      const reply = decodeMessage(
        await remote.execute(
          controller,
          encodeMessage({ type: 'execute', seq: sent, device: device.id, timeout_ms: 1000, op }),
        ),
      );
      assert.equal(reply.type, 'result');
      if (reply.type !== 'result') throw Error('Expected result');
      assert.equal(reply.seq, sent);
      return reply.completion;
    };
    // Discovery can export multiple devices; each has its own ownership lock.
    const all = await usb.Exporter.discover();
    const inventory = JSON.parse(all.inventoryJson) as { id: string }[];
    const other = inventory.find((item) => item.id !== device.id)!;
    assert.ok(other);
    const secondOpen = decodeMessage(
      await all.execute(
        'second-controller',
        encodeMessage({
          type: 'execute',
          seq: 1,
          device: other.id,
          timeout_ms: 1000,
          op: { op: 'open', configuration: 1, interface: 2, alternate: 3 },
        }),
      ),
    );
    assert.ok(
      secondOpen.type === 'result' && !secondOpen.completion.error,
      JSON.stringify(secondOpen),
    );
    await read(); // A different device's remote session cannot block this one.
    assert.equal(all.busy(other.id), true);
    assert.equal(all.busy(device.id), false);
    await all.execute(
      'second-controller',
      encodeMessage({
        type: 'execute',
        seq: 2,
        device: other.id,
        timeout_ms: 1000,
        op: { op: 'close' },
      }),
    );
    all.free();
    const open = { op: 'open', configuration: 1, interface: 2, alternate: 3 };
    const local = await acquire();
    assert.match((await request('remote', open)).error!, /in use/);
    assert.deepEqual(device.sessionOwner, { kind: 'local' });
    await local.close();
    assert.equal((await request('remote', open)).error, null);
    assert.deepEqual(device.sessionOwner, { kind: 'remote', controller: 'remote' });
    await assert.rejects(acquire(), /in use/);
    const envelope = (kind: number, size: number, token: number) => {
      const bytes = new Uint8Array(12),
        view = new DataView(bytes.buffer);
      bytes[0] = kind;
      view.setUint32(6, size);
      view.setUint16(10, token);
      return bytes;
    };
    const control = async (data: Uint8Array) =>
      request('remote', {
        op: 'control_out',
        setup: { request_type: 'vendor', recipient: 'other', request: 0, value: 0, index: 0 },
        data,
      });
    const remoteLogs = device.watchLog();
    assert.equal((await control(envelope(0xc0, 8, 0))).transferred, 12);
    assert.equal(
      (await request('remote', { op: 'transfer_in', endpoint: 0x82, length: 64 })).transferred,
      8,
    );
    assert.equal((await control(envelope(0xc4, 2, 0x1234))).transferred, 12);
    assert.deepEqual(
      (await request('remote', { op: 'transfer_in', endpoint: 0x82, length: 64 })).data,
      Uint8Array.of(0xaa, 0xbb),
    );
    assert.equal((await request('remote', { op: 'close' })).error, null);
    for (const kind of [0xc0, 0xc4]) {
      const event = (await remoteLogs.next())!;
      assert.equal(event.kind, kind);
      assert.equal(event.deviceId, device.id);
      assert.deepEqual(event.holder, { kind: 'remote', controller: 'remote' });
    }
    remoteLogs.close();
    remoteLogs.free();
    await read();
    device.stopSharing();
    assert.match((await request('remote', open)).error!, /not shared/);
    await read();
    assert.equal(fixture.log.filter((e) => e[0] === 'open').length, 2);
    assert.equal(fixture.log.filter((e) => e[0] === 'claim').length, 3);
    assert.equal(fixture.log.filter((e) => ['close', 'refused'].includes(e[0])).length, 0);
    const recorded = [];
    for (let n = 0; n < 20; n++) {
      const value = await events.next();
      recorded.push(value);
      if (!value!.sharing && !value!.sessionOwner && recorded.length > 12) break;
    }
    assert.ok(recorded.some((e) => e?.sessionOwner?.kind === 'local'));
    assert.ok(recorded.some((e) => e?.sessionOwner?.kind === 'remote'));
    device.share().free();
    assert.equal((await request('remote', open)).error, null);
    const beforeInvalid = fixture.log.length;
    assert.match(
      (await request('remote', { op: 'transfer_in', endpoint: 0x83, length: 64 })).error!,
      /Endpoint unavailable/,
    );
    assert.equal(fixture.log.length, beforeInvalid);
    assert.equal(device.status.fenced, false);
    const originalIn = fixture.device.transferIn;
    fixture.device.transferIn = async () => ({
      status: 'stall',
      data: new DataView(Uint8Array.of(1, 2, 3).buffer),
    });
    const partial = await request('remote', { op: 'transfer_in', endpoint: 0x82, length: 64 });
    assert.equal(partial.error, 'stall');
    assert.equal(partial.transferred, 3);
    assert.deepEqual(partial.data, Uint8Array.of(1, 2, 3));
    assert.equal(device.status.fenced, true);
    assert.equal((await request('remote', { op: 'close' })).error, null);
    await assert.rejects(acquire(), /completion unknown/);
    fixture.device.transferIn = originalIn;
    fixture.presence(false);
    const lost = device.watch();
    let state = await lost.next();
    while (state?.present) state = await lost.next();
    await assert.rejects(acquire(), /disconnected/);
    fixture.presence(true);
    while (!state?.present) state = await lost.next();
    await device.open();
    assert.equal(device.status.fenced, false);
    assert.equal(device.id, rediscovered.id);
    await read();
    logs.close();
    logs.free();
    lost.close();
    lost.free();
    events.close();
    events.free();
    await device.close();
    assert.equal(fixture.state.interfaceChanging, false);
    assert.equal(fixture.log.filter((e) => e[0] === 'release').length, 3);
    remote.free();
    device.free();
    rediscovered.free();
    selection.free();
    options.free();
    limits.free();
  });
} finally {
  fixture.restore();
}
