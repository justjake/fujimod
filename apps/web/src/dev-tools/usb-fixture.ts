// Test-only WebUSB device. The page still uses production Wasm and nusb.
import { installWebUsb } from '../../../../bindings/javascript/tests/webusb-fixture.ts';
export function installConsoleFixture(identity: Readonly<Partial<Record<number, string>>> = {}) {
  const fixture = installWebUsb();
  fixture.iface.alternates[0].endpoints = [
    { endpointNumber: 1, direction: 'out', type: 'bulk', packetSize: 64 },
    { endpointNumber: 2, direction: 'in', type: 'bulk', packetSize: 64 },
  ];
  const usb = (
    navigator as Navigator & {
      usb: { requestDevice(options: unknown): Promise<unknown> };
    }
  ).usb;
  const picker = usb.requestDevice.bind(usb);
  usb.requestDevice = (options: unknown) => {
    if (!navigator.userActivation.isActive)
      throw new DOMException('Device picker requires user activation', 'SecurityError');
    return picker(options);
  };
  fixture.iface.alternate = fixture.iface.alternates[0];
  const original = fixture.device.transferOut.bind(fixture.device);
  fixture.device.transferOut = async (endpoint, data) => {
    const bytes = new Uint8Array(data),
      view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (fixture.state.kind === 0xd0 && bytes.length >= 20 && view.getUint32(0, true) === 0x150000) {
      const text = `${identity[view.getUint32(16, true)] ?? 'SYNTHETIC'}\0`;
      const reply = new Uint8Array(16 + text.length * 2);
      const header = new DataView(reply.buffer);
      header.setUint32(0, 0x150000, true);
      for (let i = 0; i < text.length; i++) header.setUint16(16 + 2 * i, text.charCodeAt(i), true);
      fixture.state.pending = reply;
      fixture.log.push(['out', endpoint, bytes.slice()]);
      return { status: 'ok', bytesWritten: bytes.length };
    }
    const result = await original(endpoint, data);
    if (
      fixture.state.kind === 0xd0 &&
      bytes.length >= 20 &&
      view.getUint32(0, true) === 0x200001 &&
      view.getUint32(16, true) === 7
    ) {
      const text = new TextEncoder().encode(
        'Fuji console ready\n' + '0123456789abcdef '.repeat(28) + '\n',
      );
      const reply = new Uint8Array(32 + text.length),
        header = new DataView(reply.buffer);
      header.setUint32(0, 0x200001, true);
      header.setUint32(16, 7, true);
      header.setUint32(28, text.length, true);
      reply.set(text, 32);
      fixture.state.pending = reply;
    }
    return result;
  };
  return fixture;
}
