// Writes the setup wizard's Lottie scenes to public/setup/<name>.json:
//   node src/setup/lottie/generate.ts [--preview <text-to-lottie project dir>]
// --preview also copies each scene into a local text-to-lottie player
// (public/projects/fujimod-setup/scene-N/lottie.json) for frame checks in Skottie.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import enter from './enter.ts';

const here = dirname(fileURLToPath(import.meta.url));
const scenes: [string, { layers: unknown[]; op: number }][] = [['enter', enter as unknown as { layers: unknown[]; op: number }]];
const out = resolve(here, '../../../public/setup');
mkdirSync(out, { recursive: true });
const at = process.argv.indexOf('--preview');
const preview = at >= 0 ? process.argv[at + 1] : undefined;
scenes.forEach(([name, json], i) => {
  const text = JSON.stringify(json);
  writeFileSync(resolve(out, `${name}.json`), text);
  if (preview) {
    // the preview player loads images as files next to the scene, not embedded data
    const dir = resolve(preview, `public/projects/fujimod-setup/scene-${i + 1}`);
    mkdirSync(dir, { recursive: true });
    const copy = JSON.parse(text) as { assets?: { id: string; p: string; u: string; e: number }[] };
    for (const asset of copy.assets ?? []) {
      const m = /^data:image\/png;base64,(.*)$/.exec(asset.p);
      if (!m) continue;
      writeFileSync(resolve(dir, `${asset.id}.png`), Buffer.from(m[1], 'base64'));
      Object.assign(asset, { p: `${asset.id}.png`, u: '', e: 0 });
    }
    writeFileSync(resolve(dir, 'lottie.json'), JSON.stringify(copy));
  }
  console.log(`${name}: ${json.layers.length} layers, ${json.op} frames, ${(text.length / 1024).toFixed(1)} KB`);
});
