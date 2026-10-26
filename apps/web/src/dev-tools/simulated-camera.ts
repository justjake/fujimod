import { catalog as reference, usb, type Device } from 'fujimod';
import { CameraStore } from '../cameras/store';
import { validateCatalog, type Catalog } from '../catalog';
import { digest } from '../engine/identity';
import { WasmEngine } from '../engine/wasm-engine';
import { Exr2UsbBus } from '../engine/exr2-usb-bus';
import type { BootInput, EmulatorManifest } from 'fujimod/emulator';
import type { SimulatedCamera } from './usb-camera-model';

declare const __FUJIMOD_CATALOG__: string;
/** Cold-boot the selected owned camera in a worker; keep Dev Tools on this page. */
export async function startSimulatedCamera(id: string): Promise<SimulatedCamera> {
  const store = new CameraStore(),
    camera = (await store.list()).find((camera) => camera.id === id);
  if (!camera) throw Error('Saved camera no longer exists.');
  const response = await fetch(__FUJIMOD_CATALOG__);
  if (!response.ok) throw Error('Firmware catalog is unavailable.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find(
    (entry) => entry.id === camera.firmwareId && entry.sha256 === camera.firmwareHash,
  );
  if (!firmware?.emulate?.usb) throw Error('This saved camera has no runnable USB simulation.');
  const service = reference.selections().find((selection) => selection.firmware.id === firmware.id)
    ?.physical_controls?.boot?.service;
  if (!service) throw Error('This saved camera has no service-mode boot inputs.');
  const bootInputs: BootInput[] = service.hold.map(({ id, stage }) =>
    stage == null ? { id } : { id, stage },
  );
  if (service.usb != null) bootInputs.unshift({ id: 'usb', position: service.usb ? 'on' : 'off' });
  const bundle = new URL(firmware.emulate.usb.bundle, document.baseURI).href;
  const manifestResponse = await fetch(new URL('wasm.json', bundle));
  if (!manifestResponse.ok) throw Error('USB simulation manifest is unavailable.');
  const manifest = (await manifestResponse.json()) as EmulatorManifest;
  if (manifest.construction !== 'session' || manifest.usb?.kind !== 'exr2' || !manifest.usb.node)
    throw Error('This runtime has no USB endpoint.');
  const dat = await (await store.firmware(id)).arrayBuffer();
  if (dat.byteLength !== firmware.bytes || (await digest(dat)) !== firmware.sha256)
    throw Error('Saved firmware differs from its catalog identity.');
  let failure: Error | undefined;
  let attached: Device | undefined;
  const engine = new WasmEngine((error) => {
    failure = error;
    if (attached) usb.detachWebUsbDevice(attached);
  });
  try {
    await engine.boot(
      dat,
      {
        firmwareId: firmware.id,
        bundle,
        sceneInput: false,
        camera: { id, revision: camera.revision, modEnabled: camera.modEnabled },
        restart: true,
        initialPositions: {
          ...firmware.device?.initialPositions,
          ...manifest.usb.initialPositions,
        },
        bootInputs,
      },
      () => {},
    );
    const bus = new Exr2UsbBus((request) => {
      if (failure) return Promise.reject(failure);
      return engine.usb(request);
    }, manifest.usb.node);
    const webusb = await bus.enumerate();
    const device = await usb.attachWebUsbDevice(webusb, `simulated-${id}`);
    attached = device;
    return { device, dispose: () => engine.dispose() };
  } catch (error) {
    engine.dispose();
    throw error;
  }
}
