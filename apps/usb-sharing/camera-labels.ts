/** GitHub labels naming the camera a report came from. */
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';

export interface CameraLabel {
  name: string;
  description: string;
}

/** `X-T30 II` → `x-t30-ii`, `GFX 50R` → `gfx-50r`, `X70` → `x70`. */
export function labelName(product: string) {
  return product
    .toLowerCase()
    .replace(/\s+/g, '-')
    .replace(/[^a-z0-9-]/g, '');
}

/** One label per known firmware release: the product name, and the processor
 * when the catalog describes the body. */
export async function cameraLabels(catalog: string, processorNames: Record<string, string>) {
  const read = async (...path: string[]): Promise<unknown> =>
    JSON.parse(await readFile(join(catalog, ...path), 'utf8'));
  const processors = new Map<string, string>();
  for (const body of await readdir(join(catalog, 'firmware'))) {
    for (const version of await readdir(join(catalog, 'firmware', body))) {
      // Generated facts can precede a firmware record; such releases get no processor.
      const files = await readdir(join(catalog, 'firmware', body, version));
      if (!files.includes('firmware.json')) continue;
      const firmware = (await read('firmware', body, version, 'firmware.json')) as {
        id: string;
        body_id: string;
      };
      const { soc_id } = (await read('bodies', firmware.body_id, 'body.json')) as {
        soc_id: string;
      };
      const name = processorNames[soc_id];
      if (!name) throw Error(`No processor name for ${soc_id}`);
      processors.set(firmware.id, name);
    }
  }
  const { releases } = (await read('firmware-versions.generated.json')) as {
    releases: { firmware_id: string; product: string }[];
  };
  const labels: Record<string, CameraLabel> = {};
  for (const { firmware_id, product } of releases) {
    const processor = processors.get(firmware_id);
    labels[firmware_id] = {
      name: labelName(product),
      description: `Fujifilm ${product}${processor ? ` (${processor})` : ''}`,
    };
  }
  return labels;
}
