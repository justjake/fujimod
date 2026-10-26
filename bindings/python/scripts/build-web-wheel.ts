/** Build the browser-only Python adapter wheel; the host loads Wasm separately. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { globSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { parse } from 'smol-toml';
import { zipSync, type Zippable } from 'fflate';

const root = fileURLToPath(new URL('../', import.meta.url));
const csv = (value: string) =>
  /[",\r\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
export function build(output: string): string {
  const project = parse(readFileSync(join(root, 'pyproject.toml'), 'utf8')).project;
  assert(
    project && typeof project === 'object' && !Array.isArray(project) && !(project instanceof Date),
  );
  const version = project.version,
    requiresPython = project['requires-python'];
  assert.equal(typeof version, 'string');
  assert.equal(typeof requiresPython, 'string');
  const dist = `fujimod-${version}.dist-info`,
    source = join(root, 'python');
  const files = new Map<string, Uint8Array>();
  for (const name of globSync('fujimod/**/*.py', { cwd: source }).sort()) {
    const relative = name.replaceAll('\\', '/');
    // The emulator namespace requires native support and is not part of Pyodide.
    if (relative.startsWith('fujimod/emulator/')) continue;
    files.set(relative, readFileSync(join(source, name)));
  }
  files.set(
    `${dist}/METADATA`,
    Buffer.from(
      `Metadata-Version: 2.4\nName: fujimod\nVersion: ${version}\nSummary: FujiMod WebUSB Python adapter for Pyodide\nRequires-Python: ${requiresPython}\nLicense-File: NOTICE\n\n`,
    ),
  );
  files.set(
    `${dist}/WHEEL`,
    Buffer.from(
      'Wheel-Version: 1.0\nGenerator: fujimod-web\nRoot-Is-Purelib: true\nTag: py3-none-pyemscripten_2026_0_wasm32\n',
    ),
  );
  files.set(`${dist}/licenses/NOTICE`, readFileSync(join(root, 'NOTICE')));
  const rows = [...files].map(([name, data]) => [
    name,
    'sha256=' + createHash('sha256').update(data).digest('base64url'),
    String(data.length),
  ]);
  rows.push([`${dist}/RECORD`, '', '']);
  files.set(
    `${dist}/RECORD`,
    Buffer.from(rows.map((row) => row.map(csv).join(',') + '\r\n').join('')),
  );
  const archive: Zippable = {};
  for (const [name, data] of files)
    archive[name] = [data, { level: 6, mtime: new Date(2026, 0, 1) }];
  mkdirSync(output, { recursive: true });
  const name = `fujimod-${version}-py3-none-pyemscripten_2026_0_wasm32.whl`,
    path = join(output, name);
  writeFileSync(path, zipSync(archive));
  writeFileSync(join(output, 'manifest.json'), JSON.stringify({ wheel: name }) + '\n');
  return path;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: {
      out: { type: 'string', default: resolve(root, '../../target/web-wheels') },
      help: { type: 'boolean', short: 'h' },
    },
  });
  if (values.help)
    console.log('Usage: node bindings/python/scripts/build-web-wheel.ts [--out DIR]');
  else console.log(build(resolve(values.out)));
}
