// The WebUSB surface used by the transport. Keeping the interface structural also
// permits isolated transport tests without claiming a browser device.
export interface UsbEndpoint {
  endpointNumber: number;
  direction: string;
  type: string;
  packetSize: number;
}
export interface UsbAlternate {
  alternateSetting: number;
  endpoints: UsbEndpoint[];
}
export interface UsbInterface {
  interfaceNumber: number;
  claimed?: boolean;
  alternates: UsbAlternate[];
  alternate?: UsbAlternate;
  alternateSetting?: number;
}
export interface UsbConfiguration {
  configurationValue: number;
  interfaces: UsbInterface[];
}
export interface UsbResult {
  status: string;
  data?: DataView;
  bytesWritten?: number;
}
export interface UsbSetup {
  requestType?: string;
  recipient?: string;
  request: number;
  value?: number;
  index?: number;
}
export interface UsbDevice {
  vendorId: number;
  productId: number;
  manufacturerName?: string;
  productName?: string;
  serialNumber?: string;
  configurations: UsbConfiguration[];
  configuration?: UsbConfiguration | null;
  open(): Promise<void>;
  close(): Promise<void>;
  selectConfiguration(configuration: number): Promise<void>;
  claimInterface(iface: number): Promise<void>;
  selectAlternateInterface(iface: number, alternate: number): Promise<void>;
  controlTransferIn(setup: UsbSetup, length: number): Promise<UsbResult>;
  controlTransferOut(setup: UsbSetup, data: Uint8Array): Promise<UsbResult>;
  transferIn(endpoint: number, length: number): Promise<UsbResult>;
  transferOut(endpoint: number, data: Uint8Array): Promise<UsbResult>;
  clearHalt(direction: string, endpoint: number): Promise<void>;
}
export interface Usb {
  getDevices(): Promise<UsbDevice[]>;
}
export interface BrowserUsb extends Usb, EventTarget {
  requestDevice(options: { filters: object[] }): Promise<UsbDevice>;
}
declare global {
  interface Navigator {
    readonly usb?: BrowserUsb;
  }
}
