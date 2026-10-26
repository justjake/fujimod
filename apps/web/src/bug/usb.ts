import { initialize, usb, type Device, type Session } from 'fujimod';
import { retrieveReport, removeReport } from './transfer';
import type { ReportLink } from './report-link';
/** The camera chosen for a report, with its ff80 interfaces. */
export interface CameraChoice {
  name: string;
  interfaces: { id: number; label: string }[];
}
export interface ReportConnection {
  initialize(): Promise<void>;
  choose(): Promise<CameraChoice | undefined>;
  read(id: number, link: ReportLink, progress: (bytes: number) => void): Promise<Blob>;
  remove?(link: ReportLink): Promise<void>;
  handOff(): Promise<void>;
  onDisconnect?(callback: () => void): () => void;
  dispose(): void;
}
/** Owns one selected session; handing it to Dev Tools never chooses another device. */
export class ReportUsb implements ReportConnection {
  private device?: Device;
  private interfaces: Awaited<ReturnType<Device['interfaces']>> = [];
  private session?: Session;
  private verified?: ReportLink;
  private choice?: CameraChoice;
  async initialize() {
    await initialize();
  }
  async choose() {
    this.dispose();
    this.device = await usb.requestDevice(usb.DeviceFilter.Diagnostic);
    if (!this.device) return;
    this.interfaces = await this.device.interfaces();
    this.choice = {
      name: this.device.productName || 'Fujifilm camera',
      interfaces: this.interfaces.map((item, id) => ({
        id,
        label: `Interface ${item.interface} · Alternate ${item.alternate}`,
      })),
    };
    return this.choice;
  }
  async read(id: number, link: ReportLink, progress: (bytes: number) => void) {
    if (!this.device || !this.interfaces[id]) throw Error('Choose a camera interface.');
    const limits = new usb.FirmwareLimits(16384, 32, 16384, 4096),
      options = new usb.TransferOptions(5000, 65536);
    try {
      this.session = await usb.Session.open(this.device, this.interfaces[id], limits, options);
    } finally {
      limits.free();
      options.free();
    }
    const session = this.session;
    try {
      const report = await retrieveReport(
        {
          read: (address, count) => session.readMemory(address, count),
          write: (address, bytes) => session.writeMemory(address, bytes),
        },
        link,
        progress,
      );
      this.verified = { ...link };
      return report;
    } catch (error) {
      // No close command or mode change after an unknown/wrong camera reply.
      session.abandon();
      session.free();
      this.session = undefined;
      throw error;
    }
  }
  async remove(link: ReportLink) {
    const session = this.session;
    if (
      !this.verified ||
      ['files', 'path', 'bytes', 'crc32', 'commit'].some(
        (key) => this.verified![key as keyof ReportLink] !== link[key as keyof ReportLink],
      )
    )
      throw Error('Read and verify this report before deleting it.');
    if (!session) throw Error('Camera disconnected.');
    await removeReport(
      {
        read: (address, count) => session.readMemory(address, count),
        write: (address, bytes) => session.writeMemory(address, bytes),
      },
      link,
    );
    this.verified = undefined;
  }
  /** Close this page's report session so Dev Tools can take the camera. */
  async handOff(): Promise<void> {
    const session = this.session;
    this.session = undefined;
    try {
      await session?.close();
    } finally {
      session?.free();
      this.dispose();
    }
  }
  onDisconnect(callback: () => void) {
    const events = (navigator as Navigator & { usb?: EventTarget }).usb;
    const listener = (event: Event) => {
      const removed = (
        event as Event & { device?: { vendorId: number; productId: number; serialNumber?: string } }
      ).device;
      if (
        removed &&
        this.device &&
        removed.vendorId === this.device.vendorId &&
        removed.productId === this.device.productId &&
        removed.serialNumber === this.device.serialNumber
      )
        callback();
    };
    events?.addEventListener('disconnect', listener);
    return () => events?.removeEventListener('disconnect', listener);
  }
  dispose() {
    this.verified = undefined;
    this.session?.abandon();
    this.session?.free();
    this.session = undefined;
    for (const item of this.interfaces) item.free();
    this.interfaces = [];
    this.device?.free();
    this.device = undefined;
  }
}
