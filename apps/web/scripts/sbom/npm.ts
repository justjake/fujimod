import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { vendoredComponents } from './vendor.ts';
import { identifyLicense, licenseFiles, type Component } from './bom.ts';

interface Edges<T> {
  dependencies?: Record<string, T>;
  optionalDependencies?: Record<string, T>;
}

interface Package {
  name: string;
  version: string;
  license?: string;
}

interface Locked extends Edges<Locked> {
  devDependencies?: Record<string, Locked>;
  from: string;
  version: string;
  resolved?: string;
}

function manifest(path: string): Package {
  return JSON.parse(readFileSync(join(path, 'package.json'), 'utf8')) as Package;
}

/** Resolve packages from emitted modules and assets, including pnpm's real paths. */
export function emittedPackageDirectories(files: Iterable<string>, root: string): string[] {
  const packages = new Set<string>();
  const modules = `${sep}node_modules${sep}`;
  for (const id of files) {
    const path = resolve(root, id.split('?')[0]);
    if (!path.includes(modules) || !existsSync(path)) continue;
    let directory = dirname(realpathSync(path));
    while (directory.includes(modules)) {
      if (existsSync(join(directory, 'package.json'))) {
        const pkg = manifest(directory);
        if (pkg.name && pkg.version) {
          packages.add(directory);
          break;
        }
      }
      directory = dirname(directory);
    }
  }
  return [...packages].sort();
}

export function npmComponents(root: string, bundlePackages: string[]): Component[] {
  const projects = JSON.parse(
    execFileSync(
      'pnpm',
      ['list', '--recursive', '--depth', 'Infinity', '--json', '--lockfile-only'],
      { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 },
    ),
  ) as Locked[];
  const locked = new Map<string, Locked>();
  function index(node: Locked) {
    if (node.resolved) {
      const key = `${node.from}@${node.version}`;
      locked.set(key, node);
    }
    for (const child of Object.values({
      ...node.dependencies,
      ...node.optionalDependencies,
      ...node.devDependencies,
    }))
      index(child);
  }
  for (const project of projects) index(project);
  const components = new Map<string, Component>();
  function add(path: string, node: Locked) {
    const pkg = manifest(path);
    const key = `${pkg.name}@${pkg.version}`;
    if (components.has(key)) return;
    if (!node.resolved || pkg.version !== node.version || pkg.name !== node.from)
      throw Error(`${key}: installed package differs from pnpm-lock.yaml`);
    const declared = pkg.license ?? '';
    const explicit = /^SEE LICEN[CS]E IN (.+)$/i.exec(declared)?.[1];
    const texts = licenseFiles(
      path,
      key,
      explicit,
      join(root, 'apps/web/third_party/package-licenses/npm', pkg.name, pkg.version),
    );
    const license = explicit || !declared ? identifyLicense(texts[0], key) : declared;
    components.set(key, {
      name: pkg.name,
      version: pkg.version,
      source: node.resolved,
      license,
      texts,
      purl: `pkg:npm/${pkg.name.replace('@', '%40')}@${pkg.version}`,
      products: ['website'],
    });
  }
  for (const path of bundlePackages) {
    const pkg = manifest(path);
    const node = locked.get(`${pkg.name}@${pkg.version}`);
    if (!node)
      throw Error(`${pkg.name}@${pkg.version}: emitted package absent from pnpm-lock.yaml`);
    add(path, node);
  }
  return [...components.values(), ...vendoredComponents(root, join(root, 'apps/web'), 'website')];
}
