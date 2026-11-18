import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { LicensedComponent } from '../../src/text/license-index.ts';
import type { cycloneDx } from './bom.ts';

export function writeLicenseIndex(bom: ReturnType<typeof cycloneDx>, output: string) {
  const directory = join(output, 'client', 'licenses');
  mkdirSync(directory, { recursive: true });
  const written = new Set<string>();
  const index = bom.components.map((component): LicensedComponent => {
    const declared = component.licenses[0];
    const texts =
      'license' in declared
        ? declared.license.text.content
        : component.evidence?.licenses[0].license.text.content;
    if (!texts) throw Error(`Missing license text for ${component['bom-ref']}`);
    const filename = createHash('sha256').update(texts).digest('hex') + '.txt';
    if (!written.has(filename)) {
      writeFileSync(join(directory, filename), texts);
      written.add(filename);
    }
    return {
      name: component.name,
      version: component.version,
      source: component.externalReferences[0].url,
      license: 'license' in declared ? declared.license.id : declared.expression,
      licensePath: '/licenses/' + filename,
      products: component.properties.map(({ value }) => value),
    };
  });
  index.sort(
    (a, b) => a.name.localeCompare(b.name, 'en') || a.version.localeCompare(b.version, 'en'),
  );
  writeFileSync(join(output, 'server', 'credits-components.json'), JSON.stringify(index) + '\n');
}
