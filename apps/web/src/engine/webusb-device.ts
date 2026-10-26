/** A WebUSB device backed by an emulated USB bus. Payloads stay opaque here. */
export interface UsbSetup {
  requestType: 'standard' | 'class' | 'vendor';
  recipient: 'device' | 'interface' | 'endpoint' | 'other';
  request: number;
  value: number;
  index: number;
}
export interface UsbBus {
  controlIn(setup: UsbSetup, length: number): Promise<Uint8Array>;
  controlOut(setup: UsbSetup, data: Uint8Array): Promise<number>;
  bulkIn(endpoint: number, length: number): Promise<Uint8Array>;
  bulkOut(endpoint: number, data: Uint8Array): Promise<number>;
}
interface Endpoint {
  endpointNumber: number;
  direction: 'in' | 'out';
  type: 'bulk' | 'interrupt' | 'isochronous';
  packetSize: number;
}
interface Alternate {
  alternateSetting: number;
  interfaceClass: number;
  interfaceSubclass: number;
  interfaceProtocol: number;
  interfaceName?: string;
  endpoints: Endpoint[];
}
interface Interface {
  interfaceNumber: number;
  claimed: boolean;
  alternates: Alternate[];
  alternate: Alternate;
}
interface Configuration {
  configurationValue: number;
  configurationName?: string;
  interfaces: Interface[];
}
function bytes(data: BufferSource | Uint8Array): Uint8Array {
  return ArrayBuffer.isView(data)
    ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength).slice()
    : new Uint8Array(data).slice();
}
function view(data: Uint8Array): DataView {
  return new DataView(data.slice().buffer);
}
function configuration(raw: Uint8Array): Configuration {
  if (
    raw.length < 9 ||
    raw[0] !== 9 ||
    raw[1] !== 2 ||
    new DataView(raw.buffer, raw.byteOffset).getUint16(2, true) !== raw.length
  )
    throw Error('Firmware returned an invalid USB configuration descriptor.');
  const result: Configuration = { configurationValue: raw[5], interfaces: [] };
  let alternate: Alternate | undefined;
  for (let offset = 0; offset < raw.length; offset += raw[offset]) {
    const length = raw[offset],
      kind = raw[offset + 1];
    if (length < 2 || offset + length > raw.length) throw Error('Invalid USB descriptor extent.');
    if (kind === 4) {
      if (length < 9) throw Error('Short interface descriptor.');
      alternate = {
        alternateSetting: raw[offset + 3],
        interfaceClass: raw[offset + 5],
        interfaceSubclass: raw[offset + 6],
        interfaceProtocol: raw[offset + 7],
        endpoints: [],
      };
      let iface = result.interfaces.find((item) => item.interfaceNumber === raw[offset + 2]);
      if (!iface) {
        iface = { interfaceNumber: raw[offset + 2], claimed: false, alternate, alternates: [] };
        result.interfaces.push(iface);
      }
      iface.alternates.push(alternate);
    } else if (kind === 5) {
      if (!alternate || length < 7) throw Error('Endpoint outside an interface.');
      const address = raw[offset + 2],
        type = raw[offset + 3] & 3;
      if (type === 0) throw Error('Control endpoint in an interface.');
      alternate.endpoints.push({
        endpointNumber: address & 15,
        direction: address & 128 ? 'in' : 'out',
        type: type === 2 ? 'bulk' : type === 3 ? 'interrupt' : 'isochronous',
        packetSize: new DataView(raw.buffer, raw.byteOffset + offset).getUint16(4, true) & 2047,
      });
    }
  }
  return result;
}
export class EmulatedUsbDevice {
  readonly vendorId: number;
  readonly productId: number;
  readonly deviceClass: number;
  readonly deviceSubclass: number;
  readonly deviceProtocol: number;
  readonly deviceVersionMajor: number;
  readonly deviceVersionMinor: number;
  readonly deviceVersionSubminor: number;
  readonly usbVersionMajor: number;
  readonly usbVersionMinor: number;
  readonly usbVersionSubminor: number;
  readonly configurations: Configuration[];
  readonly manufacturerName?: string;
  readonly productName?: string;
  readonly serialNumber?: string;
  configuration: Configuration | null = null;
  opened = false;
  constructor(
    private readonly bus: UsbBus,
    descriptor: Uint8Array,
    descriptors: Uint8Array[],
    strings: Record<number, string> = {},
  ) {
    if (
      descriptor.length !== 18 ||
      descriptor[0] !== 18 ||
      descriptor[1] !== 1 ||
      descriptors.length !== descriptor[17]
    )
      throw Error('Firmware returned an invalid USB device descriptor.');
    const words = new DataView(descriptor.buffer, descriptor.byteOffset);
    this.vendorId = words.getUint16(8, true);
    this.productId = words.getUint16(10, true);
    this.deviceClass = descriptor[4];
    this.deviceSubclass = descriptor[5];
    this.deviceProtocol = descriptor[6];
    this.usbVersionMajor = descriptor[3];
    this.usbVersionMinor = descriptor[2] >>> 4;
    this.usbVersionSubminor = descriptor[2] & 15;
    this.deviceVersionMajor = descriptor[13];
    this.deviceVersionMinor = descriptor[12] >>> 4;
    this.deviceVersionSubminor = descriptor[12] & 15;
    this.configurations = descriptors.map(configuration);
    this.manufacturerName = strings[descriptor[14]];
    this.productName = strings[descriptor[15]];
    this.serialNumber = strings[descriptor[16]];
  }
  async open(): Promise<void> {
    this.opened = true;
  }
  async close(): Promise<void> {
    for (const config of this.configurations)
      for (const iface of config.interfaces) iface.claimed = false;
    this.opened = false;
  }
  private requireOpen() {
    if (!this.opened) throw new DOMException('USB device is closed.', 'InvalidStateError');
  }
  async selectConfiguration(value: number): Promise<void> {
    this.requireOpen();
    const config = this.configurations.find((item) => item.configurationValue === value);
    if (!config) throw new DOMException('Unknown USB configuration.', 'NotFoundError');
    if (this.configurations.some((c) => c.interfaces.some((i) => i.claimed)))
      throw new DOMException('An interface is claimed.', 'InvalidStateError');
    await this.bus.controlOut(
      { requestType: 'standard', recipient: 'device', request: 9, value, index: 0 },
      new Uint8Array(),
    );
    this.configuration = config;
  }
  private iface(number: number): Interface {
    this.requireOpen();
    const iface = this.configuration?.interfaces.find((item) => item.interfaceNumber === number);
    if (!iface) throw new DOMException('Unknown USB interface.', 'NotFoundError');
    return iface;
  }
  async claimInterface(number: number): Promise<void> {
    const iface = this.iface(number);
    if (iface.claimed)
      throw new DOMException('USB interface is already claimed.', 'InvalidStateError');
    iface.claimed = true;
  }
  async releaseInterface(number: number): Promise<void> {
    this.iface(number).claimed = false;
  }
  async selectAlternateInterface(number: number, value: number): Promise<void> {
    const iface = this.iface(number),
      alternate = iface.alternates.find((item) => item.alternateSetting === value);
    if (!iface.claimed || !alternate)
      throw new DOMException('USB alternate is unavailable.', 'InvalidStateError');
    await this.bus.controlOut(
      { requestType: 'standard', recipient: 'interface', request: 11, value, index: number },
      new Uint8Array(),
    );
    iface.alternate = alternate;
  }
  async controlTransferIn(setup: UsbSetup, length: number) {
    this.requireOpen();
    const data = await this.bus.controlIn(setup, length);
    if (data.length > length) return { status: 'babble', data: view(data) };
    return { status: 'ok', data: view(data) };
  }
  async controlTransferOut(setup: UsbSetup, data: BufferSource | Uint8Array = new ArrayBuffer(0)) {
    this.requireOpen();
    return { status: 'ok', bytesWritten: await this.bus.controlOut(setup, bytes(data)) };
  }
  private endpoint(number: number, direction: 'in' | 'out') {
    this.requireOpen();
    if (
      !this.configuration?.interfaces.some(
        (i) =>
          i.claimed &&
          i.alternate.endpoints.some(
            (e) => e.endpointNumber === number && e.direction === direction,
          ),
      )
    )
      throw new DOMException('USB endpoint is not claimed.', 'InvalidStateError');
  }
  async transferIn(endpoint: number, length: number) {
    this.endpoint(endpoint, 'in');
    const data = await this.bus.bulkIn(endpoint | 128, length);
    return { status: data.length > length ? 'babble' : 'ok', data: view(data) };
  }
  async transferOut(endpoint: number, data: BufferSource | Uint8Array) {
    this.endpoint(endpoint, 'out');
    return { status: 'ok', bytesWritten: await this.bus.bulkOut(endpoint, bytes(data)) };
  }
  async clearHalt(_direction: string, _endpoint: number): Promise<void> {
    throw new DOMException('Emulator endpoint halt recovery is not exposed.', 'NotSupportedError');
  }
  async reset(): Promise<void> {
    throw new DOMException('Emulator bus reset is not exposed.', 'NotSupportedError');
  }
}
