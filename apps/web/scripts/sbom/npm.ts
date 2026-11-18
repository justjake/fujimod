import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { vendoredComponents } from './vendor.ts';
import { identifyLicense, licenseFiles, type Component } from './bom.ts';

interface Edges<T> {
  dependencies?: Record<string, T>;
  optionalDependencies?: Record<string, T>;
}

interface Package extends Edges<string> {
  name: string;
  version: string;
  license?: string;
  peerDependencies?: Record<string, string>;
  peerDependenciesMeta?: Record<string, { optional?: boolean }>;
}

interface Locked extends Edges<Locked> {
  devDependencies?: Record<string, Locked>;
  path: string;
  from: string;
  version: string;
  resolved?: string;
}

function manifest(path: string): Package {
  return JSON.parse(readFileSync(join(path, 'package.json'), 'utf8')) as Package;
}

export function npmComponents(root: string, bundlePackages: string[]): Component[] {
  const projects = JSON.parse(
    execFileSync(
      'pnpm',
      ['list', '--recursive', '--depth', 'Infinity', '--json', '--lockfile-only'],
      { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 },
    ),
  ) as Locked[];
  const byPath = new Map(projects.map((project) => [project.path, project]));
  const expanded = new Map<string, Locked>();
  const locked = new Map<string, Locked>();
  function index(node: Locked) {
    if (node.dependencies || node.optionalDependencies) expanded.set(node.path, node);
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
  const visited = new Set<string>();
  const vendored: Component[] = [];
  function visit(node: Locked, optional = false) {
    if (visited.has(node.path)) return;
    if (!existsSync(join(node.path, 'package.json'))) {
      if (optional) return;
      throw Error(`Missing installed package from pnpm-lock.yaml: ${node.path}`);
    }
    visited.add(node.path);
    const workspace = byPath.get(node.path);
    const pkg = manifest(node.path);
    if (workspace) vendored.push(...vendoredComponents(root, node.path, 'website'));
    else add(node.path, node);
    const edges = workspace ?? expanded.get(node.path) ?? node;
    for (const [name, child] of Object.entries({
      ...edges.dependencies,
      ...edges.optionalDependencies,
    })) {
      if (
        name in (pkg.dependencies ?? {}) ||
        (name in (pkg.peerDependencies ?? {}) && !pkg.peerDependenciesMeta?.[name]?.optional)
      )
        visit(child);
      else if (name in (pkg.optionalDependencies ?? {})) visit(child, true);
    }
  }
  const website = byPath.get(join(root, 'apps/web'));
  if (!website) throw Error('Web project missing from pnpm-lock.yaml');
  visit(website);
  for (const path of bundlePackages) {
    const pkg = manifest(path);
    const node = locked.get(`${pkg.name}@${pkg.version}`);
    if (!node)
      throw Error(`${pkg.name}@${pkg.version}: emitted package absent from pnpm-lock.yaml`);
    add(path, node);
  }
  return [...components.values(), ...vendored];
}
