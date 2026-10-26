import assert from 'node:assert/strict';

// Shared native production-test contract for the signed lens command and the
// maximum-length device-9 exchange.
export async function checkExtendedService(
  usb: typeof import('../dist/index.js').usb,
  catalog: typeof import('../dist/index.js').catalog,
  installWebUsb: typeof import('./webusb-fixture.ts').installWebUsb,
) {
  const fixture = installWebUsb();
  const a = usb.app80;
  const id = 'synthetic-extended-service';
  const digest = 'ab'.repeat(32);
  const entries = [
    {
      code: 0x90,
      label: 'set position',
      codec: 'set_lens_position',
      slots: [0, 1, 2],
      enabled_modes: 4,
      next_mode: 0,
      firmware_kind: 1,
      handler: 0,
      parameter_table: 0,
    },
    {
      code: 0xe5,
      label: 'device9 exchange',
      codec: 'device9_exchange',
      slots: Array.from({ length: 33 }, (_, index) => index),
      enabled_modes: 4,
      next_mode: 0,
      firmware_kind: 1,
      handler: 0,
      parameter_table: 0,
    },
  ];
  const definition = catalog.service.Definition.fromJson(
    JSON.stringify({
      schema: 1,
      firmware: { id, model: 'Synthetic', version: '2', sha256: digest },
      mailbox: { page: 3, offset: 0x240, configuration_limit: 512 },
      entries,
    }),
    new catalog.FirmwareIdentity(id, digest),
  );
  const service = new a.Service(definition);
  const modes = new catalog.service.ServiceModes(4);
  const position = (value: number) => service.prepare(modes, a.Command.setLensPosition(value));

  try {
    for (const [value, bytes] of [
      [-32768, [0x80, 0, 255]],
      [32767, [0x7f, 0xff, 255]],
    ] as const) {
      assert.deepEqual(Array.from(position(value).raw.slice(1, 4)), bytes);
    }
    assert.throws(() => a.Command.setLensPosition(-32769));
    assert.throws(() => a.Command.setLensPosition(32768));

    const device9Read = service.prepare(
      modes,
      a.Command.device9(a.Device9Action.read(0x12, 0x34, new a.Device9Count(29))),
    );
    const payload = Uint8Array.from({ length: 29 }, (_, index) => 0x80 + index);
    const device9Write = service.prepare(
      modes,
      a.Command.device9(a.Device9Action.write(0x56, 0x78, new a.Device9Payload(payload))),
    );
    assert.equal(device9Read.raw.length, 34);
    assert.equal(device9Write.raw.length, 34);

    const [device] = await usb.discover(usb.DeviceFilter.Diagnostic);
    const [iface] = await device.interfaces();
    const session = await usb.Session.open(
      device,
      iface,
      new usb.FirmwareLimits(256, 256, 256, 256),
      new usb.TransferOptions(100, 1024),
    );
    const expectedRead = Array.from({ length: 29 }, (_, index) => index + 1);
    fixture.state.onConfigurationWrite = (page, address, bytes) => {
      if (page !== 3 || address !== 0x240) return;
      if (bytes[1] === 0) {
        assert.deepEqual(Array.from(bytes.slice(1, 5)), [0, 0x12, 0x34, 29]);
        for (let index = 0; index < 29; index++) {
          fixture.state.configuration.set(`3:${address + 5 + index}`, expectedRead[index]);
        }
      } else {
        assert.equal(bytes[1], 1);
        assert.deepEqual(Array.from(bytes.slice(2, 5)), [0x56, 0x78, 29]);
        assert.deepEqual(Array.from(bytes.slice(5, 34)), Array.from(payload));
      }
    };
    try {
      const readReply = await a.execute(session, device9Read);
      assert.equal(readReply.value.kind, a.ReplyKind.Device9Data);
      assert.deepEqual(Array.from(readReply.value.data), expectedRead);
      for (let index = 0; index < 29; index++) {
        assert.equal(fixture.state.configuration.get(`3:${0x245 + index}`), expectedRead[index]);
      }

      const writeReply = await a.execute(session, device9Write);
      assert.deepEqual(writeReply.value, { kind: a.ReplyKind.NoPayload });
      for (let index = 0; index < 29; index++) {
        assert.equal(fixture.state.configuration.get(`3:${0x245 + index}`), payload[index]);
      }
    } finally {
      await session.close();
    }

    const limitedSession = await usb.Session.open(
      device,
      iface,
      new usb.FirmwareLimits(256, 256, 33, 256),
      new usb.TransferOptions(100, 1024),
    );
    try {
      const before = fixture.log.length;
      assert.throws(
        () => a.execute(limitedSession, device9Read),
        (error) => usb.isError(error) && error.kind === usb.ErrorKind.InvalidArgument,
      );
      assert.equal(fixture.log.length, before);
    } finally {
      await limitedSession.close();
    }
  } finally {
    fixture.restore();
  }
}
