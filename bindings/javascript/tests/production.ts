import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import { initialize, usb, catalog } from '../dist/index.js';
import * as raw from '../pkg/fujimod.js';
await initialize({
  module_or_path: await readFile(new URL('../pkg/fujimod_bg.wasm', import.meta.url)),
});
await test('production build excludes synthetic endpoint entry points', () => {
  assert.equal('TestScript' in raw, false);
  assert.equal('TestExchange' in raw, false);
  assert.equal('TestScript' in usb, false);
  assert.equal(typeof usb.requestDevice, 'function');
});

import { installWebUsb } from './webusb-fixture.ts';
await test('production nusb/WebUSB opens explicit interfaces and preserves endpoint numbers', async () => {
  const fixture = installWebUsb();
  try {
    const devices = await usb.discover(usb.DeviceFilter.Diagnostic);
    assert.equal(devices.length, 1);
    assert.equal(devices[0].productId, 0xff80);
    const picked = await usb.requestDevice(usb.DeviceFilter.Diagnostic);
    assert.ok(picked);
    assert.equal(picked.vendorId, 0x04cb);
    assert.deepEqual(fixture.log.find((e) => e[0] === 'picker')?.[1], [
      { vendorId: 0x04cb, productId: 0xff80 },
    ]);
    const interfaces = await picked.interfaces();
    assert.equal(interfaces.length, 1);
    assert.deepEqual(
      [interfaces[0].configuration, interfaces[0].interface, interfaces[0].alternate],
      [1, 2, 3],
    );
    const trace = new usb.tracing.Trace(128, 4096),
      options = new usb.TransferOptions(100, 1024);
    options.setTrace(trace);
    const session = await usb.Session.open(
      picked,
      interfaces[0],
      new usb.FirmwareLimits(256, 256, 256, 256),
      options,
    );
    assert.equal((await session.deviceInfo(usb.InfoSelector.Product)).text, 'SYNTHETIC');
    assert.deepEqual(await session.readData(123, 5), new Uint8Array([123, 124, 125, 126, 127]));
    const upload = await usb.data.upload(
      session,
      new Uint8Array([9, 8, 7]),
      2,
      usb.DataWriteMode.Preserve,
    );
    assert.equal(upload.bytesWritten, 3);
    assert.deepEqual(fixture.state.pieces, [new Uint8Array([9, 8]), new Uint8Array([7])]);
    assert.deepEqual(await session.close(), new Uint8Array([0xaa, 0xbb]));
    assert.equal(fixture.iface.claimed, true);
    await picked.close();
    assert.equal(fixture.iface.claimed, false);
    assert.ok(fixture.log.some((e) => e[0] === 'alternate' && e[1] === 2 && e[2] === 3));
    const events = trace.drain().events;
    assert.ok(events.some((e) => e.endpoint === 0x82));
    assert.ok(events.some((e) => e.endpoint === 1));
  } finally {
    fixture.restore();
  }
});
await test('production WebUSB control deadline fences without retry or C4', async () => {
  const fixture = installWebUsb();
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      options = new usb.TransferOptions(10, 1024);
    const session = await usb.Session.open(
      device,
      new usb.InterfaceSelection(1, 2, 3),
      new usb.FirmwareLimits(256, 256, 256, 256),
      options,
    );
    fixture.state.hangControl = 0xc2;
    await assert.rejects(session.ping(), (e) => {
      assert.ok(usb.isError(e));
      assert.equal(e.kind, usb.ErrorKind.Transfer);
      assert.equal(e.stage, usb.TransferStage.Control);
      assert.equal(e.requestKind, usb.RequestKind.Ping);
      return true;
    });
    assert.equal(session.state, usb.SessionState.CompletionUnknown);
    await assert.rejects(
      async () => session.close(),
      (e) => usb.isError(e) && e.kind === usb.ErrorKind.CompletionUnknown,
    );
    assert.equal(fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xc2).length, 1);
    assert.equal(
      fixture.log.some((e) => e[0] === 'control' && e[1] === 0xc4),
      false,
    );
  } finally {
    fixture.restore();
  }
});
await test('picker cancellation returns no device and sends no USB commands', async () => {
  const fixture = installWebUsb();
  fixture.state.cancelPicker = true;
  try {
    assert.equal(await usb.requestDevice(usb.DeviceFilter.Fujifilm), undefined);
    assert.deepEqual(fixture.log, [['picker', [{ vendorId: 0x04cb }]]]);
  } finally {
    fixture.restore();
  }
});

await test('production WebUSB executes firmware-selected service requests and fences bad nested replies', async () => {
  const fixture = installWebUsb();
  try {
    const definition = catalog.service.Definition.fromJson(
      JSON.stringify({
        schema: 1,
        firmware: { id: 'synthetic-b-v2', model: 'Body B', version: '2', sha256: 'ab'.repeat(32) },
        mailbox: { page: 3, offset: 0x240, configuration_limit: 512 },
        entries: [
          {
            code: 153,
            label: 'save',
            codec: 'flush_configuration',
            slots: [7],
            enabled_modes: 4,
            next_mode: 0,
            firmware_kind: 19,
            handler: 4096,
            parameter_table: 8192,
          },
        ],
      }),
      new catalog.FirmwareIdentity('synthetic-b-v2', 'ab'.repeat(32)),
    );
    const invocation = new usb.app80.Service(definition).prepare(
      new catalog.service.ServiceModes(4),
      usb.app80.Command.flushConfiguration(),
    );
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    const open = () =>
      usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
    fixture.state.onConfigurationWrite = (page, address, bytes) => {
      if (page === 3 && address === 0x240) {
        assert.equal(bytes[0], 153);
        assert.equal(bytes[8], 255);
        fixture.state.configuration.set('3:584', 1);
      }
    };
    const session = await open(),
      reply = await usb.app80.execute(session, invocation);
    assert.deepEqual(reply.value, {
      kind: usb.app80.ReplyKind.ConfigurationSaved,
      status: { kind: usb.app80.SaveStatusKind.Failed },
    });
    assert.equal(session.state, usb.SessionState.Ready);
    await session.close();
    fixture.state.onConfigurationWrite = (page, address) => {
      if (page === 3 && address === 0x240) fixture.state.configuration.set('3:576', 0);
    };
    const bad = await open();
    await assert.rejects(
      usb.app80.execute(bad, invocation),
      (e) =>
        usb.isError(e) &&
        e.kind === usb.ErrorKind.Protocol &&
        e.protocolKind === usb.ProtocolFailureKind.CommandEcho,
    );
    assert.equal(bad.state, usb.SessionState.CompletionUnknown);
    await assert.rejects(
      bad.close(),
      (e) => usb.isError(e) && e.kind === usb.ErrorKind.CompletionUnknown,
    );
    assert.equal(fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xc4).length, 1);
    bad.abandon();
  } finally {
    fixture.restore();
  }
});

await test('production charge status one is a failed result without another host request', async () => {
  const fixture = installWebUsb();
  try {
    const definition = catalog.service.Definition.fromJson(
      JSON.stringify({
        schema: 1,
        firmware: {
          id: 'synthetic-charge-v2',
          model: 'Synthetic',
          version: '2',
          sha256: 'ab'.repeat(32),
        },
        mailbox: { page: 3, offset: 576, configuration_limit: 512 },
        entries: [
          {
            code: 153,
            label: 'charge',
            codec: 'charge_control',
            slots: [0, 1, 2],
            enabled_modes: 4,
            next_mode: 0,
            firmware_kind: 1,
            handler: 0,
            parameter_table: 0,
          },
        ],
      }),
      new catalog.FirmwareIdentity('synthetic-charge-v2', 'ab'.repeat(32)),
    );
    const invocation = new usb.app80.Service(definition).prepare(
      new catalog.service.ServiceModes(4),
      usb.app80.Command.chargeControl(usb.app80.ChargeAction.control(usb.app80.Toggle.Enabled)),
    );
    fixture.state.onConfigurationWrite = (page, address, raw) => {
      if (page === 3 && address === 576) {
        assert.equal(raw[3], 255);
        fixture.state.configuration.set('3:579', 1);
      }
    };
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    const session = await usb.Session.open(
      device,
      iface,
      new usb.FirmwareLimits(256, 256, 256, 256),
      new usb.TransferOptions(100, 1024),
    );
    const reply = await usb.app80.execute(session, invocation);
    assert.deepEqual(reply.value, {
      kind: usb.app80.ReplyKind.ChargeApplied,
      status: { kind: usb.app80.ChargeResultKind.Failed },
    });
    assert.equal(session.state, usb.SessionState.Ready);
    assert.equal(fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length, 3);
    await session.close();
  } finally {
    fixture.restore();
  }
});

await test('production audio/storage results and payload-free commands finish without extra polling', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, index, status, expected] of [
      [
        'audio_capture',
        3,
        a.Command.audioCapture(new a.AudioCount(10), a.AudioRoute.Request82),
        2,
        1,
        { kind: a.ReplyKind.AudioCaptured, status: { kind: a.AudioResultKind.Failed } },
      ],
      [
        'audio_playback',
        2,
        a.Command.audioPlayback(a.AudioPlayback.AllLanes),
        1,
        255,
        {
          kind: a.ReplyKind.AudioPlayed,
          status: { kind: a.AudioResultKind.Unknown, rawStatus: 255 },
        },
      ],
      [
        'erase_storage',
        2,
        a.Command.eraseStorage(a.StorageErase.ResetProfiles),
        1,
        1,
        { kind: a.ReplyKind.StorageErased, status: { kind: a.StorageEraseResultKind.Failed } },
      ],
      [
        'storage_status',
        1,
        a.Command.storageStatus(),
        0,
        0,
        { kind: a.ReplyKind.StorageStatus, status: { kind: a.StorageStatusKind.Zero } },
      ],
      ['format_storage', 0, a.Command.formatStorage(), null, null, { kind: a.ReplyKind.NoPayload }],
      ['display_setup', 0, a.Command.displaySetup(), null, null, { kind: a.ReplyKind.NoPayload }],
    ] as const) {
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'synthetic-storage-v2',
            model: 'Synthetic',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots: Array.from({ length: count }, (_, n) => 7 + n),
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('synthetic-storage-v2', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      fixture.state.onConfigurationWrite = (page, address, raw) => {
        if (page === 3 && address === 576 && index !== null) {
          assert.equal(raw[8 + index], 255);
          fixture.state.configuration.set(`3:${584 + index}`, status);
        }
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
  } finally {
    fixture.restore();
  }
});

await test('production radio and NAND results do not request host completion polling', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, index, byte, expected] of [
      [
        'nand_read_blocks',
        3,
        a.Command.nandReadBlocks(a.NandSpare.Include, 255),
        2,
        1,
        { kind: a.ReplyKind.NandRead, status: { kind: a.NandReadResultKind.RejectedOrWaitError } },
      ],
      [
        'option41',
        1,
        a.Command.option41(),
        0,
        1,
        { kind: a.ReplyKind.Option41, state: { kind: a.Option41StateKind.Zero } },
      ],
      [
        'wlan_control',
        1,
        a.Command.wlanControl(a.Toggle.Enabled),
        null,
        null,
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'radio_control',
        4,
        a.Command.radioControl(
          a.RadioAction.primary(
            a.RadioMode.Mode1,
            new a.RadioChannel(14),
            new a.RadioPower(12),
            a.Toggle.Enabled,
          ),
        ),
        null,
        null,
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'signal_strength',
        7,
        a.Command.signalStrength(new a.PeerAddress(new Uint8Array(6))),
        6,
        255,
        { kind: a.ReplyKind.SignalStrength, rawValue: 255 },
      ],
      ['iperf', 1, a.Command.iperf(), 0, 1, { kind: a.ReplyKind.Iperf, rawResult: 1 }],
    ] as const) {
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: { id: 'radio-b', model: 'Synthetic', version: '2', sha256: 'ab'.repeat(32) },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots: Array.from({ length: count }, (_, n) => n + 7),
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('radio-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      fixture.state.onConfigurationWrite = (page, address, raw) => {
        if (page === 3 && address === 576 && index !== null) {
          assert.equal(raw[8 + index], 255);
          fixture.state.configuration.set(`3:${584 + index}`, byte);
        }
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
  } finally {
    fixture.restore();
  }
});

await test('production WPS and station snapshot bytes never trigger another host request', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, output, expected] of [
      [
        'wps',
        8,
        a.Command.wps(a.WpsAction.start(a.WpsTarget.broadcast())),
        { 7: 1 },
        { kind: a.ReplyKind.WpsCallback, snapshot: { kind: a.WpsCallbackResultKind.Timeout } },
      ],
      [
        'wps',
        8,
        a.Command.wps(a.WpsAction.publishProgress()),
        { 7: 19 },
        {
          kind: a.ReplyKind.WpsProgress,
          snapshot: {
            kind: a.WpsProgressKind.State,
            phase: a.WpsPhase.CredentialsReceived,
            connection: a.ConnectionState.Connected,
          },
        },
      ],
      ['wps', 8, a.Command.wps(a.WpsAction.stop()), {}, { kind: a.ReplyKind.NoPayload }],
      ['wps', 8, a.Command.wps(a.WpsAction.connect()), {}, { kind: a.ReplyKind.NoPayload }],
      [
        'station',
        4,
        a.Command.station(a.StationAction.startStored()),
        { 2: 255 },
        { kind: a.ReplyKind.StationConnection, rawResult: 255 },
      ],
      [
        'station',
        4,
        a.Command.station(a.StationAction.scan(a.StationScanMode.Mode1)),
        { 2: 1, 3: 255 },
        { kind: a.ReplyKind.StationScan, rawResult: 1, rawCount: 255 },
      ],
      [
        'station',
        4,
        a.Command.station(a.StationAction.stop()),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
    ] as const) {
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: { id: 'wps-b', model: 'Synthetic', version: '2', sha256: 'ab'.repeat(32) },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots: Array.from({ length: count }, (_, n) => n + 7),
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('wps-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      fixture.state.onConfigurationWrite = (page, address, raw) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(output)) {
            assert.equal(raw[8 + Number(index)], 255);
            fixture.state.configuration.set(`3:${584 + Number(index)}`, byte);
          }
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
  } finally {
    fixture.restore();
  }
});

await test('production AP and transfer controls complete once and retain unknown result sentinels', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, outputs, expected] of [
      [
        'network_reconfigure',
        14,
        a.Command.networkReconfigure(a.NetworkReconfigureAction.readConfiguration()),
        { 8: 0, 9: 1, 10: 128, 11: 254, 12: 255, 13: 42 },
        { kind: a.ReplyKind.NetworkConfiguration, data: new Uint8Array([0, 1, 128, 254, 255, 42]) },
      ],
      [
        'network_reconfigure',
        14,
        a.Command.networkReconfigure(
          a.NetworkReconfigureAction.apply(
            255,
            new a.RadioChannel(0),
            a.AccessPointRadioMode.Mode2,
            a.AccessPointSecurity.Option0,
            new a.CountryBytes(new Uint8Array([85, 83])),
          ),
        ),
        {},
        { kind: a.ReplyKind.NetworkConfiguration, data: new Uint8Array(6).fill(255) },
      ],
      [
        'network_reconfigure',
        14,
        a.Command.networkReconfigure(a.NetworkReconfigureAction.stopServer()),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_prepare',
        1,
        a.Command.transferPrepare(),
        { 0: 3 },
        {
          kind: a.ReplyKind.TransferPrepared,
          result: { kind: a.TransferPrepareResultKind.Status7 },
        },
      ],
      [
        'transfer_select_operation',
        2,
        a.Command.transferSelectOperation(a.TransferOperation.Operation6),
        { 1: 255 },
        {
          kind: a.ReplyKind.TransferOperationSelected,
          result: { kind: a.TransferResultKind.Unknown, rawStatus: 255 },
        },
      ],
      [
        'transfer_ready',
        1,
        a.Command.transferReady(),
        { 0: 1 },
        { kind: a.ReplyKind.TransferReadiness, result: { kind: a.TransferResultKind.Failed } },
      ],
      [
        'transfer_select_channel',
        2,
        a.Command.transferSelectChannel(a.TransferChannel.Operation4),
        { 1: 1 },
        {
          kind: a.ReplyKind.TransferChannelSelected,
          result: { kind: a.TransferResultKind.Failed },
        },
      ],
      [
        'transfer_start',
        1,
        a.Command.transferStart(),
        { 0: 0 },
        { kind: a.ReplyKind.TransferStarted, result: { kind: a.TransferResultKind.Success } },
      ],
    ] as const) {
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'ap-transfer-b',
            model: 'Synthetic',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots: Array.from({ length: count }, (_, n) => n + 7),
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('ap-transfer-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      fixture.state.onConfigurationWrite = (page, address, raw) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(outputs)) {
            assert.equal(raw[8 + Number(index)], 255);
            fixture.state.configuration.set(`3:${584 + Number(index)}`, byte);
          }
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
  } finally {
    fixture.restore();
  }
});

await test('production transfer payload commands use one exchange for raw, rejected and unknown data', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    const word = new a.TransferWordAddress(4),
      half = new a.TransferHalfwordAddress(0xffffffff),
      payload = new a.TransferPayloadByte(255);
    for (const [codec, count, command, outputs, expected] of [
      [
        'transfer_word',
        10,
        a.Command.transferReadWord(word),
        { 5: 137, 6: 171, 7: 205, 8: 239, 9: 0 },
        {
          kind: a.ReplyKind.TransferWordRead,
          result: { kind: a.TransferWordReadResultKind.Value, rawValue: 0x89abcdef },
        },
      ],
      [
        'transfer_word',
        10,
        a.Command.transferReadWord(word),
        { 9: 1 },
        {
          kind: a.ReplyKind.TransferWordRead,
          result: { kind: a.TransferWordReadResultKind.Rejected },
        },
      ],
      [
        'transfer_word',
        10,
        a.Command.transferWriteWord(word, 0),
        { 9: 255 },
        {
          kind: a.ReplyKind.TransferWordWritten,
          result: { kind: a.TransferResultKind.Unknown, rawStatus: 255 },
        },
      ],
      [
        'transfer_pair',
        7,
        a.Command.transferPair(new a.TransferPairFirst(511), 65535),
        { 6: 3 },
        {
          kind: a.ReplyKind.TransferPair,
          result: { kind: a.TransferPairResultKind.CallbackError },
        },
      ],
      [
        'transfer_halfword',
        7,
        a.Command.transferReadHalfword(half),
        { 5: 137, 6: 171 },
        { kind: a.ReplyKind.TransferHalfword, rawValue: 0x89ab },
      ],
      [
        'transfer_halfword',
        7,
        a.Command.transferWriteHalfword(half, 0),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_option_word',
        6,
        a.Command.transferReadOptionWord(a.TransferOption.Option7),
        { 2: 137, 3: 171, 4: 205, 5: 239 },
        { kind: a.ReplyKind.TransferOptionWord, rawValue: 0x89abcdef },
      ],
      [
        'transfer_byte',
        3,
        a.Command.transferByte(payload),
        { 1: 0, 2: 255 },
        { kind: a.ReplyKind.TransferBytes, data: new Uint8Array([0, 255]) },
      ],
      [
        'transfer_mode_byte',
        3,
        a.Command.transferModeByte(payload),
        { 1: 255, 2: 0 },
        { kind: a.ReplyKind.TransferBytes, data: new Uint8Array([255, 0]) },
      ],
      [
        'transfer_bitfields',
        5,
        a.Command.transferBitfields(payload, 65535, 0),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_selected_word',
        4,
        a.Command.transferSetSelectedWord(a.TransferSelectedWordKind.Kind9, 65535),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_word7',
        3,
        a.Command.transferSetWord7(a.TransferWord7Kind.Kind7, 65535),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_word16',
        3,
        a.Command.transferSetWord16(a.TransferWord16Kind.Kind16, 65535),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'transfer_load_test',
        1,
        a.Command.transferLoadTest(a.TransferLoadMode.Run),
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
    ] as const) {
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'transfer-data-b',
            model: 'Synthetic',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots: Array.from({ length: count }, (_, n) => n + 7),
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('transfer-data-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(outputs))
            fixture.state.configuration.set(`3:${584 + Number(index)}`, byte);
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
  } finally {
    fixture.restore();
  }
});

await test('configuration commands preserve raw snapshots and execute one mailbox exchange', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, input, outputs, expected] of [
      [
        'configuration_flags',
        11,
        a.Command.readConfigurationFlags(a.ConfigurationBank.Bank1, a.Toggle.Enabled),
        [0, 1, 1, ...Array(8).fill(255)],
        { 3: 0, 4: 1, 5: 127, 6: 128, 7: 253, 8: 254, 9: 255, 10: 42 },
        {
          kind: a.ReplyKind.ConfigurationFlags,
          rawData: new Uint8Array([0, 1, 127, 128, 253, 254, 255, 42]),
        },
      ],
      [
        'configuration_flags',
        11,
        a.Command.readConfigurationFlags(a.ConfigurationBank.Bank0, a.Toggle.Disabled),
        [0, 0, 0, ...Array(8).fill(255)],
        {},
        { kind: a.ReplyKind.ConfigurationFlags, rawData: new Uint8Array(8).fill(255) },
      ],
      [
        'configuration_flags',
        11,
        a.Command.applyConfigurationFlags(a.ConfigurationBank.Bank1, a.Toggle.Disabled),
        [1, 1, 0, ...Array(8).fill(0)],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'configuration_mode',
        2,
        a.Command.selectConfigurationMode(a.Toggle.Enabled, a.ConfigurationMode.Mode3),
        [1, 3],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'configuration_pack_options',
        4,
        a.Command.packConfigurationOptions(a.Toggle.Disabled, a.Toggle.Enabled, 255, 0),
        [0, 1, 255, 0],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'configuration_read_options',
        4,
        a.Command.readConfigurationOptions(),
        [255, 255, 255, 255],
        { 0: 137, 1: 171, 2: 205, 3: 239 },
        { kind: a.ReplyKind.ConfigurationOptions, rawValue: 0x89abcdef },
      ],
      [
        'configuration_read_options',
        4,
        a.Command.readConfigurationOptions(),
        [255, 255, 255, 255],
        {},
        { kind: a.ReplyKind.ConfigurationOptions, rawValue: 0xffffffff },
      ],
    ] as const) {
      const slots = Array.from({ length: count }, (_, i) => (i * 5 + 7) % 32);
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: { id: 'config-b', model: 'Synthetic', version: '2', sha256: 'ab'.repeat(32) },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots,
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('config-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      assert.deepEqual(
        slots.map((i) => invocation.raw[1 + i]),
        input,
      );
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(outputs))
            fixture.state.configuration.set(`3:${577 + slots[Number(index)]}`, byte);
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => a.Command.readConfigurationFlags(99, a.Toggle.Enabled));
    assert.throws(() =>
      a.Command.packConfigurationOptions(a.Toggle.Enabled, a.Toggle.Disabled, 256, 0),
    );
  } finally {
    fixture.restore();
  }
});

await test('battery and display controls use firmware slots without invented result semantics', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, input] of [
      ['battery_control', 2, a.Command.batteryControl(a.BatteryAction.ResetPending), [0, 0]],
      ['battery_control', 2, a.Command.batteryControl(a.BatteryAction.CallbackWait), [1, 0]],
      ['initialize_display', 1, a.Command.initializeDisplay(), [0]],
      [
        'display_pattern',
        5,
        a.Command.displayPattern(a.DisplayPatternAction.resetPreset()),
        [0, 0, 0, 0, 0],
      ],
      [
        'display_pattern',
        5,
        a.Command.displayPattern(a.DisplayPatternAction.bars()),
        [1, 0, 0, 0, 0],
      ],
      [
        'display_pattern',
        5,
        a.Command.displayPattern(a.DisplayPatternAction.solid(18, 52, 86)),
        [2, 0, 18, 86, 52],
      ],
    ] as const) {
      const slots = Array.from({ length: count }, (_, i) => (i * 5 + 7) % 32);
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'peripheral-b',
            model: 'Synthetic',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots,
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('peripheral-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      assert.deepEqual(
        slots.map((i) => invocation.raw[1 + i]),
        input,
      );
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576)
          for (const slot of slots) fixture.state.configuration.set(`3:${577 + slot}`, 255);
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, {
        kind: a.ReplyKind.NoPayload,
      });
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => a.Command.batteryControl(99));
    assert.throws(() => a.DisplayPatternAction.solid(256, 0, 0));
  } finally {
    fixture.restore();
  }
});

await test('adjustment and lens requests preserve state snapshots without host completion instructions', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, input, outputs, expected] of [
      [
        'adjustment_configuration',
        5,
        a.Command.adjustmentConfiguration(a.AdjustmentConfigurationAction.readStatus()),
        [0, 0, 0, 0, 255],
        { 4: 1 },
        {
          kind: a.ReplyKind.AdjustmentConfiguration,
          state: { kind: a.AdjustmentConfigurationStateKind.Configured },
        },
      ],
      [
        'adjustment_configuration',
        5,
        a.Command.adjustmentConfiguration(a.AdjustmentConfigurationAction.readStatus()),
        [0, 0, 0, 0, 255],
        {},
        {
          kind: a.ReplyKind.AdjustmentConfiguration,
          state: { kind: a.AdjustmentConfigurationStateKind.Unknown, rawStatus: 255 },
        },
      ],
      [
        'adjustment_configuration',
        5,
        a.Command.adjustmentConfiguration(
          a.AdjustmentConfigurationAction.apply(
            a.AdjustmentProfile.Profile6,
            new a.AdjustmentDuration(15360),
          ),
        ),
        [1, 6, 255, 60, 0],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'select_adjustment_mode',
        1,
        a.Command.selectAdjustmentMode(a.AdjustmentMode.Mode8),
        [1],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'select_adjustment_mode',
        1,
        a.Command.selectAdjustmentMode(a.AdjustmentMode.Mode16),
        [0],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'lens_select',
        5,
        a.Command.lensSelect(a.LensTarget.initialize()),
        [0, 0, 255, 255, 255],
        { 2: 0, 3: 0, 4: 0 },
        {
          kind: a.ReplyKind.LensSelection,
          range: { kind: a.LensSelectionRangeKind.NotRejected },
          rawDetail: new Uint8Array([0, 0]),
        },
      ],
      [
        'lens_select',
        5,
        a.Command.lensSelect(a.LensTarget.index(new a.LensSelectionIndex(254))),
        [255, 0, 255, 255, 255],
        { 2: 1, 3: 0, 4: 0 },
        {
          kind: a.ReplyKind.LensSelection,
          range: { kind: a.LensSelectionRangeKind.OutOfRange },
          rawDetail: new Uint8Array([0, 0]),
        },
      ],
      [
        'lens_select',
        5,
        a.Command.lensSelect(a.LensTarget.initialize()),
        [0, 0, 255, 255, 255],
        {},
        {
          kind: a.ReplyKind.LensSelection,
          range: { kind: a.LensSelectionRangeKind.Unknown, rawStatus: 255 },
          rawDetail: new Uint8Array([255, 255]),
        },
      ],
      [
        'lens_operation2',
        1,
        a.Command.lensOperation2(),
        [255],
        { 0: 0 },
        { kind: a.ReplyKind.LensOperation, rawValue: 0 },
      ],
      [
        'lens_operation3',
        1,
        a.Command.lensOperation3(),
        [255],
        { 0: 1 },
        { kind: a.ReplyKind.LensOperation, rawValue: 1 },
      ],
      ['lens_operation4', 0, a.Command.lensOperation4(), [], {}, { kind: a.ReplyKind.NoPayload }],
      ['lens_operation5', 0, a.Command.lensOperation5(), [], {}, { kind: a.ReplyKind.NoPayload }],
    ] as const) {
      const slots = Array.from({ length: count }, (_, i) => (i * 5 + 7) % 32);
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: { id: 'requests-b', model: 'Synthetic', version: '2', sha256: 'ab'.repeat(32) },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots,
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('requests-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      assert.deepEqual(
        slots.map((i) => invocation.raw[1 + i]),
        input,
      );
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(outputs))
            fixture.state.configuration.set(`3:${577 + slots[Number(index)]}`, byte);
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
    assert.throws(() => new a.AdjustmentDuration(0));
    assert.throws(() => new a.AdjustmentDuration(15361));
    assert.throws(() => new a.LensSelectionIndex(160));
    assert.throws(() => new a.LensSelectionIndex(255));
  } finally {
    fixture.restore();
  }
});

await test('direct adjustment requests publish data without turning callback bytes into host waits', async () => {
  const fixture = installWebUsb(),
    a = usb.app80;
  try {
    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic),
      [iface] = await device.interfaces();
    for (const [codec, count, command, input, outputs, expected] of [
      ['lens_operation6', 0, a.Command.lensOperation6(), [], {}, { kind: a.ReplyKind.NoPayload }],
      [
        'publish_lens_completion_parameter',
        2,
        a.Command.publishLensCompletionParameter(),
        [255, 255],
        { 0: 1, 1: 255 },
        {
          kind: a.ReplyKind.LensCompletionParameter,
          flag: { kind: a.LensCompletionParameterFlagKind.Parameter80 },
          encodedValue: 255,
        },
      ],
      [
        'publish_lens_completion_parameter',
        2,
        a.Command.publishLensCompletionParameter(),
        [255, 255],
        {},
        {
          kind: a.ReplyKind.LensCompletionParameter,
          flag: { kind: a.LensCompletionParameterFlagKind.Unknown, rawStatus: 255 },
          encodedValue: 255,
        },
      ],
      [
        'prepare_adjustment_output',
        0,
        a.Command.prepareAdjustmentOutput(),
        [],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'prepare_alternate_adjustment_output',
        0,
        a.Command.prepareAlternateAdjustmentOutput(),
        [],
        {},
        { kind: a.ReplyKind.NoPayload },
      ],
      [
        'lens_bounded_position',
        3,
        a.Command.lensBoundedPosition(18, 52),
        [18, 52, 255],
        { 2: 1 },
        { kind: a.ReplyKind.LensPosition, range: { kind: a.LensPositionRangeKind.OutOfRange } },
      ],
      [
        'lens_bounded_position',
        3,
        a.Command.lensBoundedPosition(0, 255),
        [0, 255, 255],
        { 2: 0 },
        { kind: a.ReplyKind.LensPosition, range: { kind: a.LensPositionRangeKind.NotRejected } },
      ],
      [
        'lens_fixed_position',
        1,
        a.Command.lensFixedPosition(),
        [255],
        { 0: 0 },
        { kind: a.ReplyKind.LensOperation, rawValue: 0 },
      ],
      [
        'invoke_primary_parameter_callback',
        2,
        a.Command.invokePrimaryParameterCallback(255),
        [255, 255],
        { 1: 1 },
        { kind: a.ReplyKind.AdjustmentCallback, rawValue: 1 },
      ],
      [
        'invoke_secondary_parameter_callback',
        2,
        a.Command.invokeSecondaryParameterCallback(18),
        [18, 255],
        {},
        { kind: a.ReplyKind.AdjustmentCallback, rawValue: 255 },
      ],
      [
        'query_adjustment_option',
        4,
        a.Command.queryAdjustmentOption(new a.AdjustmentOptionByte(255)),
        [255, 255, 255, 255],
        { 1: 137, 2: 171, 3: 1 },
        {
          kind: a.ReplyKind.AdjustmentOption,
          status: { kind: a.AdjustmentOptionStatusKind.ReportedFailure },
          rawValue: 0x89ab,
        },
      ],
      [
        'query_adjustment_option',
        4,
        a.Command.queryAdjustmentOption(new a.AdjustmentOptionByte(0)),
        [0, 255, 255, 255],
        {},
        {
          kind: a.ReplyKind.AdjustmentOption,
          status: { kind: a.AdjustmentOptionStatusKind.Unknown, rawStatus: 255 },
          rawValue: 65535,
        },
      ],
    ] as const) {
      const slots = Array.from({ length: count }, (_, i) => (i * 5 + 7) % 32);
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'adjustment-b',
            model: 'Synthetic',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
          entries: [
            {
              code: 153,
              label: codec,
              codec,
              slots,
              enabled_modes: 4,
              next_mode: 0,
              firmware_kind: 1,
              handler: 0,
              parameter_table: 0,
            },
          ],
        }),
        new catalog.FirmwareIdentity('adjustment-b', 'ab'.repeat(32)),
      );
      const invocation = new a.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        command,
      );
      assert.deepEqual(
        slots.map((i) => invocation.raw[1 + i]),
        input,
      );
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576)
          for (const [index, byte] of Object.entries(outputs))
            fixture.state.configuration.set(`3:${577 + slots[Number(index)]}`, byte);
      };
      const before = fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length;
      const session = await usb.Session.open(
        device,
        iface,
        new usb.FirmwareLimits(256, 256, 256, 256),
        new usb.TransferOptions(100, 1024),
      );
      assert.deepEqual((await a.execute(session, invocation)).value, expected);
      assert.equal(session.state, usb.SessionState.Ready);
      assert.equal(
        fixture.log.filter((e) => e[0] === 'control' && e[1] === 0xd0).length - before,
        3,
      );
      await session.close();
    }
    // @ts-expect-error Runtime boundary deliberately exercises invalid JavaScript input.
    assert.throws(() => a.Command.queryAdjustmentOption(new a.TransferPayloadByte(1)));
    assert.throws(() => a.Command.lensBoundedPosition(256, 0));
  } finally {
    fixture.restore();
  }
});

import { checkExtendedService } from './extended-service.ts';
await test('production signed fields and full device9 exchanges validate boundaries', async () => {
  await checkExtendedService(usb, catalog, installWebUsb);
});

await test('external WebUSB attachment reuses one Rust owner without navigator discovery', async () => {
  const fixture = installWebUsb();
  const [first, second] = await Promise.all([
    usb.attachWebUsbDevice(fixture.device, 'attached-camera'),
    usb.attachWebUsbDevice(fixture.device, 'attached-camera'),
  ]);
  try {
    assert.equal(first.id, 'attached-camera');
    assert.equal(second.id, first.id);
    assert.equal(fixture.log.filter(([kind]) => kind === 'open').length, 1);
    assert.equal(fixture.log.some(([kind]) => kind === 'list' || kind === 'picker'), false);
    const selections = await first.interfaces(), limits = new usb.FirmwareLimits(256, 256, 256, 256), options = new usb.TransferOptions(1000, 1024);
    try {
      const session = await first.acquireSession(selections[0], limits, options, 'Attached identity');
      try {
        assert.deepEqual(second.sessionOwner, { kind: 'local', tool: 'Attached identity' });
        const reply = await session.deviceInfo(usb.InfoSelector.Product);
        assert.equal(reply.text, 'SYNTHETIC');
        reply.free();
        await session.close();
      } finally { session.free(); }
      assert.equal(second.sessionOwner, null);
      await first.close();
      await second.open();
      assert.equal(fixture.log.filter(([kind]) => kind === 'open').length, 1);
      await second.close();
      const events = second.watch();
      try {
        assert.equal((await events.next())?.present, true);
        usb.detachWebUsbDevice(first);
        assert.equal(second.present, false);
        assert.equal((await events.next())?.present, false);
      } finally { events.close(); events.free(); }
    } finally {
      selections.forEach((selection) => selection.free()); limits.free(); options.free();
    }
  } finally { first.free(); second.free(); fixture.restore(); }
});
