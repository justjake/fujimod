import type { WasmRequest } from '@fujimod/emulator-ui/engine/wasm-worker-protocol';
import type { UsbStatus } from 'fujimod/emulator';
import { EmulatedUsbDevice, type UsbBus, type UsbSetup } from './webusb-device';

type Request = Extract<WasmRequest, { node: string }>;
/** A host policy over opaque controller packets; never interprets vendor payloads. */
export class Exr2UsbBus implements UsbBus {
  private tail: Promise<unknown> = Promise.resolve();
  constructor(
    private readonly call: (request: Request) => Promise<unknown>,
    private readonly node: string,
    private readonly timeoutMs = 10_000,
  ) {}
  private serialize<T>(run: () => Promise<T>): Promise<T> {
    const result = this.tail.then(run);
    this.tail = result.catch(() => {});
    return result;
  }
  private async status(step = false): Promise<UsbStatus['endpoint']> {
    const state = (await this.call({
      action: step ? 'usb_step' : 'usb_status',
      node: this.node,
    })) as UsbStatus;
    if (!state.connected)
      throw new DOMException('Emulated USB cable disconnected.', 'NetworkError');
    return state.endpoint;
  }
  private async wait(predicate: (state: UsbStatus['endpoint']) => boolean, label: string) {
    const deadline = performance.now() + this.timeoutMs;
    let state = await this.status();
    while (!predicate(state)) {
      if (performance.now() >= deadline)
        throw new DOMException(`Emulated USB timeout: ${label}`, 'TimeoutError');
      state = await this.status(true);
    }
    return state;
  }
  private drained(state: UsbStatus['endpoint']) {
    const reg = (offset: number) => state.registers[offset / 4];
    return (
      [
        [0x2c, 0x40],
        [0x30, 0x44],
        [0x34, 0x48],
      ].every(([flag, mask]) => !(reg(flag) & ~reg(mask) & 0xffff)) &&
      !(reg(0x40) & 0x2000) &&
      !(reg(0x48) & 2)
    );
  }
  private async setup(setup: UsbSetup, input: boolean, length: number) {
    if (!Number.isSafeInteger(length) || length < 0 || length > 65535)
      throw Error('Invalid control length.');
    await this.wait((s) => this.drained(s), 'previous control');
    // A host inter-transaction gap lets the interrupt tail retire after its
    // controller acknowledgment, without inspecting guest code or task state.
    await this.status(true);
    const bytes = new Uint8Array(8),
      view = new DataView(bytes.buffer);
    bytes[0] =
      (input ? 128 : 0) |
      { standard: 0, class: 32, vendor: 64 }[setup.requestType] |
      { device: 0, interface: 1, endpoint: 2, other: 3 }[setup.recipient];
    bytes[1] = setup.request;
    view.setUint16(2, setup.value, true);
    view.setUint16(4, setup.index, true);
    view.setUint16(6, length, true);
    await this.call({ action: 'usb_setup', node: this.node, bytes });
    await this.wait((s) => s.setup_position === 8 && !(s.registers[0x2c / 4] & 0x2000), 'setup');
  }
  private async take(endpoint: number, size: number): Promise<Uint8Array> {
    const deadline = performance.now() + this.timeoutMs;
    for (;;) {
      const s = await this.status(),
        port = s.options.in_endpoints.find(([ep]) => ep === endpoint)?.[1];
      if (port === undefined) throw Error('Unknown USB IN endpoint.');
      if (s.tx_packets[port])
        return this.call({
          action: 'usb_in',
          node: this.node,
          endpoint,
          size,
        }) as Promise<Uint8Array>;
      if (port === 1 && !(s.registers[0x30 / 4] & 4))
        await this.call({ action: 'usb_in_token', node: this.node, endpoint });
      if (performance.now() >= deadline)
        throw new DOMException('Emulated USB timeout: IN packet', 'TimeoutError');
      await this.status(true);
    }
  }
  controlIn(setup: UsbSetup, length: number): Promise<Uint8Array> {
    return this.serialize(async () => {
      if (length <= 0) throw Error('Control IN requires a positive length.');
      await this.setup(setup, true, length);
      const data: number[] = [];
      while (data.length < length) {
        const packet = await this.take(0x80, 64);
        data.push(...packet);
        if (packet.length < 64) break;
      }
      if (data.length > length) throw Error('Guest control response exceeds request.');
      await this.wait((s) => this.drained(s), 'control IN acknowledgment');
      await this.call({ action: 'usb_out', node: this.node, endpoint: 0, bytes: new Uint8Array() });
      await this.wait((s) => this.drained(s), 'control status OUT');
      return Uint8Array.from(data);
    });
  }
  controlOut(setup: UsbSetup, data: Uint8Array): Promise<number> {
    return this.serialize(async () => {
      await this.setup(setup, false, data.length);
      for (let offset = 0; offset < data.length; offset += 64) {
        const bytes = data.slice(offset, offset + 64);
        await this.call({ action: 'usb_out', node: this.node, endpoint: 0, bytes });
        await this.wait((s) => s.rx_position[0] === bytes.length && this.drained(s), 'control OUT');
      }
      if ((await this.take(0x80, 64)).length)
        throw Error('Guest control status IN is not a zero-length packet.');
      await this.wait((s) => this.drained(s), 'control status IN');
      return data.length;
    });
  }
  bulkOut(endpoint: number, data: Uint8Array): Promise<number> {
    return this.serialize(async () => {
      const s = await this.status(),
        port = s.options.out_endpoints.find(([ep]) => ep === endpoint)?.[1];
      if (port === undefined) throw Error('Unknown USB OUT endpoint.');
      for (let offset = 0; offset < data.length; offset += s.options.packet_bytes) {
        const bytes = data.slice(offset, offset + s.options.packet_bytes);
        await this.wait((s) => s.rx_position[port] >= s.rx_length[port], 'bulk OUT ready');
        await this.call({ action: 'usb_out', node: this.node, endpoint, bytes });
        await this.wait((s) => s.rx_position[port] >= bytes.length, 'bulk OUT consumed');
      }
      return data.length;
    });
  }
  bulkIn(endpoint: number, length: number): Promise<Uint8Array> {
    return this.serialize(async () => {
      if (!Number.isSafeInteger(length) || length <= 0)
        throw Error('USB IN requires a positive length.');
      const size = (await this.status()).options.packet_bytes,
        data = new Uint8Array(length);
      let offset = 0;
      while (offset < length) {
        const count = Math.min(size, length - offset),
          packet = await this.take(endpoint, count);
        data.set(packet, offset);
        offset += packet.length;
        if (packet.length < count) break;
      }
      return data.slice(0, offset);
    });
  }
  async enumerate(): Promise<EmulatedUsbDevice> {
    await this.wait((s) => (s.registers[0x90 / 4] & 6) === 6, 'controller enable');
    await this.call({ action: 'usb_reset', node: this.node });
    await this.wait((s) => this.drained(s), 'bus reset');
    const descriptor = (value: number, length: number, index = 0) =>
      this.controlIn(
        { requestType: 'standard', recipient: 'device', request: 6, value, index },
        length,
      );
    const device = await descriptor(0x100, 18);
    if (device.length !== 18 || device[0] !== 18 || device[1] !== 1)
      throw Error('Invalid guest device descriptor.');
    await this.controlOut(
      { requestType: 'standard', recipient: 'device', request: 5, value: 1, index: 0 },
      new Uint8Array(),
    );
    const configurations: Uint8Array[] = [];
    for (let index = 0; index < device[17]; index++) {
      const header = await descriptor(0x200 | index, 9);
      if (header.length !== 9 || header[1] !== 2)
        throw Error('Invalid guest configuration header.');
      configurations.push(
        await descriptor(0x200 | index, new DataView(header.buffer).getUint16(2, true)),
      );
    }
    const strings: Record<number, string> = {};
    if (device.slice(14, 17).some(Boolean)) {
      const languages = await descriptor(0x300, 255);
      if (languages.length < 4 || languages[1] !== 3)
        throw Error('Invalid guest language descriptor.');
      const language = new DataView(languages.buffer).getUint16(2, true);
      for (const index of new Set(device.slice(14, 17)))
        if (index) {
          const raw = await descriptor(0x300 | index, 255, language);
          if (raw.length < 2 || raw[0] !== raw.length || raw[1] !== 3 || raw.length % 2)
            throw Error('Invalid guest string descriptor.');
          strings[index] = new TextDecoder('utf-16le', { fatal: true }).decode(raw.subarray(2));
        }
    }
    return new EmulatedUsbDevice(this, device, configurations, strings);
  }
}
