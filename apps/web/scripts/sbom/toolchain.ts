import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { type Component, type Product } from './bom.ts';

export function rustStandardLibrary(root: string, product: Product, toolchain?: string): Component {
  const rustc = (...args: string[]) =>
    execFileSync('rustc', [...(toolchain ? [`+${toolchain}`] : []), ...args], {
      cwd: root,
      encoding: 'utf8',
    }).trim();
  const info = rustc('--version', '--verbose');
  const commit = /^commit-hash: ([0-9a-f]{40})$/m.exec(info)?.[1];
  const version = /^release: (\S+)$/m.exec(info)?.[1];
  if (!commit || !version) throw Error('Cannot identify the Rust standard library revision');
  const docs = join(rustc('--print', 'sysroot'), 'share/doc/rust');
  return {
    name: 'Rust standard library',
    version,
    source: `https://github.com/rust-lang/rust/tree/${commit}/library`,
    license: 'Apache-2.0 OR MIT',
    texts: ['licenses/Apache-2.0.txt', 'licenses/MIT.txt', 'COPYRIGHT-library.html'].map((name) =>
      readFileSync(join(docs, name), 'utf8'),
    ),
    products: [product],
  };
}

export function emscriptenRuntime(root: string, expected: string): Component {
  const run = (command: string, ...args: string[]) =>
    execFileSync(command, args, { cwd: root, encoding: 'utf8' }).trim();
  const info = run('emcc', '--version');
  const revision = /^emcc .* (\S+) \(([0-9a-f]{40})\)$/m.exec(info);
  if (!revision || revision[1] !== expected)
    throw Error(`Emscripten license source does not match the linked runtime (${expected})`);
  const directory = run('em-config', 'EMSCRIPTEN_ROOT');
  return {
    name: 'Emscripten runtime and system libraries',
    version: revision[1],
    source: `https://github.com/emscripten-core/emscripten/tree/${revision[2]}`,
    license: '(MIT OR NCSA) AND MIT AND (Apache-2.0 WITH LLVM-exception)',
    texts: [
      'LICENSE',
      'system/lib/libc/musl/COPYRIGHT',
      'system/lib/compiler-rt/LICENSE.TXT',
      'system/lib/libcxx/LICENSE.TXT',
      'system/lib/libcxxabi/LICENSE.TXT',
      'system/lib/libunwind/LICENSE.TXT',
    ].map((name) => readFileSync(join(directory, name), 'utf8')),
    products: ['emulator'],
  };
}
