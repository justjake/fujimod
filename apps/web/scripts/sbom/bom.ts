import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';

export type Product = 'camera' | 'website' | 'emulator';

export interface Component {
  name: string;
  version: string;
  source: string;
  license: string;
  texts: string[];
  products: Product[];
  purl?: string;
}

export function licenseFiles(
  directory: string,
  name: string,
  explicit?: string,
  fallback?: string,
): string[] {
  const names = explicit
    ? [explicit]
    : readdirSync(directory, { withFileTypes: true })
        .filter(
          (entry) =>
            (entry.isFile() || entry.isSymbolicLink()) &&
            /^(?:licen[cs]es?|copying|ofl|unlicense)(?:$|[-_.])/i.test(entry.name),
        )
        .map((entry) => entry.name)
        .sort();
  const texts = names
    .filter((file) => existsSync(resolve(directory, file)))
    .map((file) => readFileSync(resolve(directory, file), 'utf8'))
    .filter((text) => text.trim());
  if (!texts.length && fallback && existsSync(fallback)) return licenseFiles(fallback, name);
  if (!texts.length) throw Error(`${name}: no local license file in ${directory}`);
  return texts;
}

export function identifyLicense(text: string, component: string) {
  if (text.includes('Permission is hereby granted, free of charge')) return 'MIT';
  if (/Apache License\s+Version 2\.0/.test(text)) return 'Apache-2.0';
  if (/SIL OPEN FONT LICENSE\s+Version 1\.1/.test(text)) return 'OFL-1.1';
  if (text.includes('ISC License')) return 'ISC';
  throw Error(`${component}: local license file has no detectable SPDX identifier`);
}

export function mergeComponents(components: Iterable<Component>) {
  const unique = new Map<string, Component>();
  for (const component of components) {
    const { name, version, source, license, texts, products } = component;
    if (!name || !version || !/^https?:\/\//.test(source) || !products.length)
      throw Error(`${name}: missing component provenance`);
    if (
      !license ||
      /^(?:UNLICENSED|NONE|NOASSERTION)$/i.test(license) ||
      !texts.length ||
      texts.some((text) => !text.trim())
    )
      throw Error(`${name}@${version}: missing SPDX license or license text`);
    const key = component.purl ?? `${name}@${version}:${source}`;
    const previous = unique.get(key);
    if (
      previous &&
      (previous.license !== license ||
        previous.source !== source ||
        JSON.stringify(previous.texts) !== JSON.stringify(texts))
    )
      throw Error(`${key}: conflicting license or source information`);
    unique.set(key, {
      ...component,
      products: [...new Set([...(previous?.products ?? []), ...products])].sort(),
    });
  }
  return unique;
}

export function cycloneDx(components: Iterable<Component>) {
  const unique = mergeComponents(components);
  return {
    $schema: 'http://cyclonedx.org/schema/bom-1.6.schema.json',
    bomFormat: 'CycloneDX',
    specVersion: '1.6',
    version: 1,
    metadata: { component: { type: 'application', name: 'FujiMod' } },
    components: [...unique.entries()]
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([ref, component]) => {
        const { name, version, source, license, purl, texts, products } = component;
        const text = { contentType: 'text/plain', content: texts.join('\n\n') };
        const single = /^[A-Za-z0-9.+-]+$/.test(license) && !license.startsWith('LicenseRef-');
        return {
          'bom-ref': ref,
          type: 'library',
          name,
          version,
          ...(purl ? { purl } : {}),
          licenses: single
            ? [{ license: { id: license, text, acknowledgement: 'declared' } }]
            : [{ expression: license, acknowledgement: 'declared' }],
          ...(!single
            ? { evidence: { licenses: [{ license: { name: 'Upstream license texts', text } }] } }
            : {}),
          externalReferences: [{ type: purl ? 'distribution' : 'vcs', url: source }],
          properties: products.map((value) => ({ name: 'fujimod:product', value })),
        };
      }),
  };
}
