import { validateCatalog, type Catalog } from '../catalog';

declare const __FUJIMOD_CATALOG__: string;
/** Load the camera workflow table selected by a checked firmware identity. */
export async function cameraDefinition(firmwareId: string) {
  const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
  if (!response.ok) throw Error('Could not load the firmware catalog.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId);
  if (!firmware?.camera_table) throw Error('The selected firmware has no camera table.');
  const table = await fetch(new URL(firmware.camera_table, document.baseURI));
  if (!table.ok) throw Error('Could not load the camera table.');
  return [await table.text(), firmware.id, firmware.sha256] as const;
}

/** Load the built reader and its checked metadata before starting USB work. */
export async function flashReader(firmwareId: string) {
  const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
  if (!response.ok) throw Error('Could not load the firmware catalog.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId);
  if (!firmware?.flash_reader) throw Error('The selected firmware has no built flash reader.');
  const url = new URL(firmware.flash_reader, document.baseURI);
  const metadata = await fetch(url);
  if (!metadata.ok) throw Error('Could not load the flash reader metadata.');
  const artifact = (await metadata.json()) as { payload: unknown; image: string };
  if (!/^[a-zA-Z0-9_-]+\.bin$/.test(artifact.image)) throw Error('Invalid flash reader artifact.');
  const image = await fetch(new URL(artifact.image, url));
  if (!image.ok) throw Error('Could not load the flash reader image.');
  return [JSON.stringify(artifact.payload), new Uint8Array(await image.arrayBuffer())] as const;
}
