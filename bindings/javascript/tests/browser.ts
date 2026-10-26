import { stripTypeScriptTypes } from 'node:module';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
const files = new Map<string, [string, string]>([
  ['/dist/index.js', ['../dist/index.js', 'text/javascript']],
  ['/pkg/fujimod.js', ['../pkg/fujimod.js', 'text/javascript']],
  ['/pkg/service-values.js', ['../pkg/service-values.js', 'text/javascript']],
  ['/pkg/fujimod_bg.wasm', ['../pkg/fujimod_bg.wasm', 'application/wasm']],
  ['/tests/webusb-fixture.js', ['./webusb-fixture.ts', 'text/javascript']],
]);
const server = createServer(async (req, res) => {
  const path = new URL(req.url ?? '/', 'http://localhost').pathname;
  if (path === '/') {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><title>FujiMod browser tests</title>');
    return;
  }
  const entry =
    files.get(path) ??
    (path.startsWith('/pkg/snippets/') && path.endsWith('.js')
      ? ['../' + path.slice(1), 'text/javascript']
      : undefined);
  if (!entry) {
    res.writeHead(404).end();
    return;
  }
  try {
    const data = await readFile(new URL(entry[0], import.meta.url));
    res.writeHead(200, { 'content-type': entry[1] });
    res.end(entry[0].endsWith('.ts') ? stripTypeScriptTypes(data.toString()) : data);
  } catch {
    res.writeHead(500).end();
  }
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const address = server.address();
assert.ok(address && typeof address === 'object');
let browser;
try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : {}),
  });
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${address.port}/`);
  const result = await page.evaluate(async () => {
    const { usb, catalog, initialize } = (await import(
      String('/dist/index.js')
    )) as typeof import('../dist/index.js');
    const { installWebUsb } = (await import(
      String('/tests/webusb-fixture.js')
    )) as typeof import('./webusb-fixture.ts');
    await initialize();
    const fixture = installWebUsb();
    try {
      const device = await usb.requestDevice(usb.DeviceFilter.Diagnostic);
      if (!device) throw Error('Picker returned no device');
      const interfaces = await device.interfaces(),
        options = new usb.TransferOptions(20, 1024),
        trace = new usb.tracing.Trace(128, 4096);
      options.setTrace(trace);
      const session = await usb.Session.open(
        device,
        interfaces[0],
        new usb.FirmwareLimits(256, 256, 256, 256),
        options,
      );
      const product = (await session.deviceInfo(usb.InfoSelector.Product)).text;
      const data = [...(await session.readData(123, 5))];
      const definition = catalog.service.Definition.fromJson(
        JSON.stringify({
          schema: 1,
          firmware: {
            id: 'synthetic-b-v2',
            model: 'Body B',
            version: '2',
            sha256: 'ab'.repeat(32),
          },
          mailbox: { page: 3, offset: 576, configuration_limit: 512 },
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
      fixture.state.onConfigurationWrite = (page, address) => {
        if (page === 3 && address === 576) fixture.state.configuration.set('3:584', 1);
      };
      const invocation = new usb.app80.Service(definition).prepare(
        new catalog.service.ServiceModes(4),
        usb.app80.Command.flushConfiguration(),
      );
      const serviceValue = (await usb.app80.execute(session, invocation)).value;
      const typedSaveFailed =
        serviceValue.kind === usb.app80.ReplyKind.ConfigurationSaved &&
        serviceValue.status.kind === usb.app80.SaveStatusKind.Failed;
      const acknowledgment = [...(await session.close())];
      const closed = session.state === usb.SessionState.Closed;
      const traceEndpoints = trace.drain().events.map((e) => e.endpoint);
      const second = await usb.Session.open(
        device,
        interfaces[0],
        new usb.FirmwareLimits(256, 256, 256, 256),
        options,
      );
      fixture.state.hangControl = 0xc2;
      let timeout = false;
      try {
        await second.ping();
      } catch (error) {
        timeout =
          usb.isError(error) &&
          error.kind === usb.ErrorKind.Transfer &&
          error.stage === usb.TransferStage.Control;
      }
      const unknown = second.state === usb.SessionState.CompletionUnknown;
      second.abandon();
      return {
        secure: isSecureContext,
        realWindow: window instanceof Window,
        typedSaveFailed,
        product,
        data,
        acknowledgment,
        closed,
        traceEndpoints,
        timeout,
        unknown,
        controls: fixture.log.filter((e) => e[0] === 'control').map((e) => e[1]),
      };
    } finally {
      fixture.restore();
    }
  });
  assert.equal(result.typedSaveFailed, true);
  assert.equal(result.secure, true);
  assert.equal(result.realWindow, true);
  assert.equal(result.product, 'SYNTHETIC');
  assert.deepEqual(result.data, [123, 124, 125, 126, 127]);
  assert.deepEqual(result.acknowledgment, [0xaa, 0xbb]);
  assert.equal(result.closed, true);
  assert.ok(result.traceEndpoints.includes(0x82));
  assert.ok(result.traceEndpoints.includes(1));
  assert.equal(result.timeout, true);
  assert.equal(result.unknown, true);
  assert.equal(result.controls.filter((kind) => kind === 0xc4).length, 1);
  console.log(
    'PASS Chromium: picker, descriptors, identification, bulk data, typed service invocation, raw close, tracing and timeout fencing (mock WebUSB; no hardware)',
  );
} finally {
  await browser?.close();
  server.close();
  await once(server, 'close');
}
