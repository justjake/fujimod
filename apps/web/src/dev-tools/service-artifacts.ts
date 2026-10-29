import { validateCatalog, type Catalog } from '../catalog';
import { generationFailure } from './generation';

declare const __FUJIMOD_CATALOG__: string;
/** Load the generated service workflow selected by a checked firmware identity. */
export async function serviceWorkflow(
  firmwareId: string,
  capability: 'setup' | 'persistence' = 'setup',
) {
  const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
  if (!response.ok) throw Error('Could not load the firmware catalog.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId);
  if (!firmware?.service_workflow) {
    throw Error(generationFailure(firmwareId, 'service'));
  }
  const responseWorkflow = await fetch(new URL(firmware.service_workflow, document.baseURI));
  if (!responseWorkflow.ok) throw Error('Could not load the service workflow.');
  const text = await responseWorkflow.text();
  const facts = JSON.parse(text) as { setup?: unknown; persistence?: unknown };
  if (!facts?.setup || (capability === 'persistence' && !facts.persistence)) {
    const operation = capability === 'persistence' ? 'Persistent debug mode' : 'Service setup';
    throw Error(
      `${operation} facts are missing from the workflow for ${firmware.model} ${firmware.version}.`,
    );
  }
  return [text, firmware.id, firmware.sha256] as const;
}

/** Load the built reader and its checked metadata before starting USB work. */
export async function flashReader(firmwareId: string) {
  const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
  if (!response.ok) throw Error('Could not load the firmware catalog.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId);
  if (!firmware?.flash_reader) throw Error(generationFailure(firmwareId, 'flash'));
  const url = new URL(firmware.flash_reader, document.baseURI);
  const metadata = await fetch(url);
  if (!metadata.ok) throw Error('Could not load the flash reader metadata.');
  const artifact = (await metadata.json()) as { payload: unknown; image: string };
  if (!/^[a-zA-Z0-9_-]+\.bin$/.test(artifact.image)) throw Error('Invalid flash reader artifact.');
  const image = await fetch(new URL(artifact.image, url));
  if (!image.ok) throw Error('Could not load the flash reader image.');
  return [JSON.stringify(artifact.payload), new Uint8Array(await image.arrayBuffer())] as const;
}
