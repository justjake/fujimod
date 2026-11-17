import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { type Component, type Product } from './bom.ts';

interface Upstream {
  name: string;
  license: string;
  files: (string | { file: string; header: true })[];
  version?: string;
  source?: string;
  submodule?: string;
}

export function vendoredComponents(root: string, directory: string, product: Product): Component[] {
  const path = join(directory, 'upstream.json');
  if (!existsSync(path)) {
    if (
      readdirSync(directory, { withFileTypes: true }).some(
        (entry) => entry.isDirectory() && /^(?:vendor|upstream|third[-_]party)$/.test(entry.name),
      )
    )
      throw Error(`${directory}: missing upstream.json for vendored sources`);
    return [];
  }
  const packages = JSON.parse(readFileSync(path, 'utf8')) as Upstream[];
  return packages.map((pkg) => {
    let { version, source } = pkg;
    const upstream = pkg.submodule ? resolve(root, pkg.submodule) : directory;
    if (pkg.submodule) {
      const git = (cwd: string, ...args: string[]) =>
        execFileSync('git', args, { cwd, encoding: 'utf8' }).trim();
      const pinned = git(root, 'ls-tree', 'HEAD', '--', pkg.submodule);
      const revision = /^160000 commit ([0-9a-f]{40})\t/.exec(pinned)?.[1];
      if (!revision || git(upstream, 'rev-parse', 'HEAD') !== revision)
        throw Error(`${pkg.name}: submodule does not match the pinned revision`);
      const config = (key: string) =>
        git(root, 'config', '--file', '.gitmodules', ...key.split(' '));
      const pathKey = config('--get-regexp ^submodule\\..*\\.path$')
        .split('\n')
        .find((line) => line.slice(line.indexOf(' ') + 1) === pkg.submodule)
        ?.split(' ')[0];
      const url = pathKey && config(`--get ${pathKey.replace(/\.path$/, '.url')}`);
      if (!url) throw Error(`${pkg.name}: submodule has no upstream URL`);
      version ??= revision;
      source = `${url.replace(/\.git$/, '')}/tree/${revision}`;
    }
    if (!version || !source) throw Error(`${pkg.name}: missing pinned upstream version or source`);
    const texts = pkg.files.map((notice) => {
      const file = typeof notice === 'string' ? notice : notice.file;
      const text = readFileSync(resolve(upstream, file), 'utf8');
      if (typeof notice === 'string') return text;
      const comment = /^\s*\/\*[\s\S]*?\*\//.exec(text)?.[0];
      if (!comment || !/permission|redistribution/i.test(comment))
        throw Error(`${pkg.name}: no license in the header of ${file}`);
      return comment;
    });
    return {
      name: pkg.name,
      license: pkg.license,
      version,
      source,
      texts,
      products: [product],
    };
  });
}
