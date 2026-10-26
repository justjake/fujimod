// Camera-side WebUSB fixture for the real nusb backend. Node has no USB access.
const assert = {
  equal(actual: unknown, expected: unknown) {
    if (!Object.is(actual, expected))
      throw new Error(`Expected ${String(expected)}, received ${String(actual)}`);
  },
  ok(value: unknown) {
    if (!value) throw new Error('Fixture assertion failed');
  },
  deepEqual(actual: Record<string, unknown>, expected: Record<string, unknown>) {
    const keys = Object.keys(expected);
    if (
      !actual ||
      Object.keys(actual).length !== keys.length ||
      keys.some((key) => !Object.is(actual[key], expected[key]))
    )
      throw new Error('Unexpected WebUSB control parameters');
  },
};
const utf16le = (text: string) => {
  const bytes = new Uint8Array(text.length * 2),
    view = new DataView(bytes.buffer);
  text.split('').forEach((ch, i) => view.setUint16(i * 2, ch.charCodeAt(0), true));
  return bytes;
};
const words = (...values: number[]) => {
  const bytes = new Uint8Array(values.length * 4),
    v = new DataView(bytes.buffer);
  values.forEach((n, i) => v.setUint32(i * 4, n, true));
  return bytes;
};
const descriptor = new Uint8Array([
  18, 1, 0, 2, 0, 0, 0, 64, 0xcb, 4, 0x80, 0xff, 0, 1, 1, 2, 3, 1,
]);
const configDescriptor = new Uint8Array([
  9, 2, 32, 0, 1, 1, 0, 0x80, 50, 9, 4, 2, 3, 2, 0xff, 0, 0, 0, 7, 5, 0x82, 2, 64, 0, 0, 7, 5, 1, 2,
  64, 0, 0,
]);
const view = (bytes: ArrayLike<number>) => new DataView(Uint8Array.from(bytes).buffer);
export function installWebUsb() {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'Window');
  const nav = globalThis.navigator;
  const previousUsb = Object.getOwnPropertyDescriptor(nav, 'usb');
  const realWindow = typeof Window !== 'undefined' && globalThis instanceof Window;
  type LogEntry = [string, ...unknown[]];
  interface State {
    pending: Uint8Array;
    kind?: number;
    cancelPicker: boolean;
    interfaceDelay: number;
    interfaceChanging: boolean;
    hangControl?: number;
    pieces: Uint8Array[];
    memory: Map<string, number>;
    configuration: Map<string, number>;
    onConfigurationWrite?: (
      page: number,
      address: number,
      bytes: Uint8Array,
    ) => void | Promise<void>;
  }
  const log: LogEntry[] = [],
    state: State = {
      pending: new Uint8Array(),
      kind: undefined,
      cancelPicker: false,
      interfaceDelay: 0,
      interfaceChanging: false,
      hangControl: undefined,
      pieces: [],
      memory: new Map(),
      configuration: new Map(),
    };
  const refuseOverlap = (operation: string) => {
    if (!state.interfaceChanging) return;
    log.push(['refused', operation]);
    throw new DOMException(
      `Failed to execute '${operation}' on 'USBDevice': An operation that changes interface state is in progress.`,
      'InvalidStateError',
    );
  };
  const interfaceChange = async (operation: string, action: () => void) => {
    refuseOverlap(operation);
    state.interfaceChanging = true;
    try {
      if (state.interfaceDelay)
        await new Promise((resolve) => setTimeout(resolve, state.interfaceDelay));
      action();
    } finally {
      state.interfaceChanging = false;
    }
  };
  type Alternate = {
    alternateSetting: number;
    interfaceClass: number;
    interfaceSubclass: number;
    interfaceProtocol: number;
    interfaceName: string;
    endpoints?: { endpointNumber: number; direction: string; type: string; packetSize: number }[];
  };
  const iface: {
    interfaceNumber: number;
    claimed: boolean;
    alternates: Alternate[];
    alternate?: Alternate;
  } = {
    interfaceNumber: 2,
    claimed: false,
    alternates: [
      {
        alternateSetting: 3,
        interfaceClass: 0xff,
        interfaceSubclass: 0,
        interfaceProtocol: 0,
        interfaceName: 'fixture',
      },
    ],
  };
  const config = { configurationValue: 1, interfaces: [iface] };
  const device = {
    vendorId: 0x04cb,
    productId: 0xff80,
    deviceVersionMajor: 1,
    deviceVersionMinor: 0,
    deviceVersionSubminor: 0,
    usbVersionMajor: 2,
    usbVersionMinor: 0,
    usbVersionSubminor: 0,
    deviceClass: 0,
    deviceSubclass: 0,
    deviceProtocol: 0,
    manufacturerName: 'FUJIFILM',
    productName: 'Synthetic service device',
    serialNumber: 'synthetic',
    configuration: null as typeof config | null,
    configurations: [config],
    opened: false,
    async close() {
      refuseOverlap('close');
      log.push(['close']);
      this.opened = false;
      iface.claimed = false;
    },
    async open() {
      refuseOverlap('open');
      this.opened = true;
      log.push(['open']);
    },
    async selectConfiguration(number: number) {
      assert.equal(number, 1);
      await interfaceChange('selectConfiguration', () => {
        this.configuration = config;
      });
      log.push(['configuration', number]);
    },
    async claimInterface(number: number) {
      assert.equal(number, 2);
      await interfaceChange('claimInterface', () => {
        iface.claimed = true;
      });
      log.push(['claim', number]);
    },
    async selectAlternateInterface(number: number, alternate: number) {
      assert.equal(number, 2);
      assert.equal(alternate, 3);
      await interfaceChange('selectAlternateInterface', () => {
        iface.alternate = iface.alternates.find((item) => item.alternateSetting === alternate);
      });
      log.push(['alternate', number, alternate]);
    },
    async releaseInterface(number: number) {
      assert.equal(number, 2);
      await interfaceChange('releaseInterface', () => {
        iface.claimed = false;
      });
      log.push(['release', number]);
    },
    async controlTransferIn(
      setup: { requestType: string; recipient: string; request: number; value: number },
      length: number,
    ) {
      assert.equal(setup.requestType, 'standard');
      assert.equal(setup.recipient, 'device');
      assert.equal(setup.request, 6);
      log.push(['descriptor', setup.value, length]);
      const bytes = setup.value >>> 8 === 1 ? descriptor : configDescriptor;
      return { status: 'ok', data: view(bytes.slice(0, length)) };
    },
    async controlTransferOut(setup: Record<string, unknown>, raw: ArrayBuffer) {
      assert.deepEqual(setup, {
        requestType: 'vendor',
        recipient: 'other',
        request: 0,
        value: 0,
        index: 0,
      });
      const bytes = new Uint8Array(raw),
        v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const kind = bytes[0],
        offset = v.getUint32(2),
        size = v.getUint32(6),
        token = v.getUint16(10);
      assert.equal(bytes.length, 12);
      assert.equal(bytes[1], 0);
      assert.equal(token, kind === 0xc0 ? 0 : 0x1234);
      log.push(['control', kind, size, offset]);
      state.kind = kind;
      if (kind === state.hangControl) return new Promise(() => {});
      if (kind === 0xc0) state.pending = new Uint8Array([0x34, 0x12, 0, 0, 0, 0, 1, 0]);
      else if (kind === 0xc2 || kind === 0xc4) state.pending = new Uint8Array([0xaa, 0xbb]);
      else if (kind === 0xe1)
        state.pending = Uint8Array.from({ length: size }, (_, i) => (offset + i) & 255);
      else if (kind === 0xe8) state.pending = new Uint8Array([0x01, 0x02, 0x03, 0x04]);
      else assert.ok(kind === 0xd0 || kind === 0xd1 || kind === 0xe0);
      return { status: 'ok', bytesWritten: bytes.length };
    },
    async transferOut(endpoint: number, raw: ArrayBuffer) {
      assert.equal(endpoint, 1);
      const bytes = Uint8Array.from(new Uint8Array(raw));
      log.push(['out', endpoint, bytes]);
      if (state.kind === 0xe0) state.pieces.push(bytes);
      else {
        assert.equal(state.kind, 0xd0);
        const v = new DataView(bytes.buffer),
          code = v.getUint32(0, true);
        if (code === 0x150000) {
          const selector = v.getUint32(16, true);
          assert.ok([4, 0xf0001, 0xf0002].includes(selector));
          const text = utf16le(
            (selector === 4 ? 'SYNTHETIC' : selector === 0xf0001 ? 'WRITING' : 'NORMAL') + '\0',
          );
          state.pending = new Uint8Array(16 + text.length);
          state.pending.set(words(code, 0, 0, 0));
          state.pending.set(text, 16);
        } else if (code === 0x760001 || code === 0x550001)
          state.pending = words(code, 0, 0, 0, 0, 0, 0, 0);
        else if (code === 0x450006) state.pending = words(code, 1, 0xffff, 0, 9, 8, 7, 6);
        else if (code === 0x580000)
          state.pending = words(code, 0, 0, 0, 0, 48, 0x5a5a5a5a, 0x5a5a5a5a);
        else if (code === 0xdeadbeef) {
          state.pending = bytes.slice();
          const reply = new DataView(state.pending.buffer);
          reply.setUint32(4, 1, true);
          reply.setUint32(8, 0xffff, true);
        } else if (code === 0x200001 || code === 0x1a0002) {
          const fn = v.getUint32(16, true);
          if (code === 0x1a0002 && fn === 1) {
            state.pending = bytes.slice(0, 36);
            const reply = new DataView(state.pending.buffer);
            reply.setUint32(28, 4096, true);
            reply.setUint32(32, 256, true);
          } else if (fn === 2 || fn === 3) {
            const address = v.getUint32(28, true),
              length = v.getUint32(32, true),
              page = code === 0x1a0002 ? v.getUint32(24, true) : 0;
            const map = code === 0x200001 ? state.memory : state.configuration;
            const key = (offset: number) => `${page}:${address + offset}`;
            if (fn === 3) {
              for (let i = 0; i < length; i++) map.set(key(i), bytes[36 + i]);
              state.pending = bytes.slice(0, 36);
              if (code === 0x1a0002)
                await state.onConfigurationWrite?.(page, address, bytes.slice(36));
            } else {
              state.pending = new Uint8Array(36 + length);
              state.pending.set(bytes.slice(0, 36));
              for (let i = 0; i < length; i++)
                state.pending[36 + i] = map.get(key(i)) ?? (address + i) & 255;
            }
          } else if (code === 0x200001 && fn === 5) state.pending = bytes.slice(0, 36);
          else if (code === 0x200001 && fn === 6) state.pending = words(code, 0, 0, 0, 6, 0, 0, 0);
          else if (code === 0x200001 && fn === 7)
            state.pending = new Uint8Array([...words(code, 0, 0, 0, 7, 42, 7, 3), 108, 111, 103]);
          else if (code === 0x200001 && fn === 1)
            state.pending = new Uint8Array([...words(code, 0, 0, 0, 1, 7, 3), 108, 111, 103]);
          else throw new Error(`Unexpected fixture function ${fn}`);
        } else throw new Error(`Unexpected fixture command ${code.toString(16)}`);
      }
      return { status: 'ok', bytesWritten: bytes.length };
    },
    async transferIn(endpoint: number, maximum: number) {
      assert.equal(endpoint, 2);
      assert.equal(maximum % 64, 0);
      log.push(['in', endpoint, maximum]);
      const raw = state.pending.slice(0, maximum);
      state.pending = state.pending.slice(raw.length);
      return { status: 'ok', data: view(raw) };
    },
    async reset() {
      throw new Error('Unexpected USB reset');
    },
  };
  const otherInterface = { ...iface, claimed: false };
  const otherConfig = { ...config, interfaces: [otherInterface] };
  const other = {
    ...device,
    vendorId: 0x1234,
    productId: 0x5678,
    configuration: null as typeof otherConfig | null,
    configurations: [otherConfig],
    async selectConfiguration(number: number) {
      assert.equal(number, 1);
      this.configuration = otherConfig;
    },
    async claimInterface(number: number) {
      assert.equal(number, 2);
      otherInterface.claimed = true;
      log.push(['claim', number]);
    },
    async releaseInterface(number: number) {
      assert.equal(number, 2);
      otherInterface.claimed = false;
      log.push(['release', number]);
    },
  };
  if (!realWindow)
    Object.defineProperty(globalThis, 'Window', {
      configurable: true,
      value: class Window {
        static [Symbol.hasInstance](object: unknown) {
          return object === globalThis;
        }
      },
    });
  const usbEvents = new EventTarget();
  Object.defineProperty(nav, 'usb', {
    configurable: true,
    value: {
      addEventListener: usbEvents.addEventListener.bind(usbEvents),
      removeEventListener: usbEvents.removeEventListener.bind(usbEvents),
      async getDevices() {
        log.push(['list']);
        return [other, device];
      },
      async requestDevice(options: { filters: unknown }) {
        log.push(['picker', options.filters]);
        if (state.cancelPicker) throw new DOMException('Canceled', 'NotFoundError');
        return device;
      },
    },
  });
  return {
    device,
    presence(present: boolean) {
      if (!present) { device.opened = false; device.configuration = null; iface.claimed = false; }
      const event = new Event(present ? 'connect' : 'disconnect');
      Object.defineProperty(event, 'device', { value: device });
      usbEvents.dispatchEvent(event);
    },
    iface,
    state,
    log,
    restore() {
      if (previousWindow) Object.defineProperty(globalThis, 'Window', previousWindow);
      else Reflect.deleteProperty(globalThis, 'Window');
      if (previousUsb) Object.defineProperty(nav, 'usb', previousUsb);
      else Reflect.deleteProperty(nav, 'usb');
    },
  };
}
