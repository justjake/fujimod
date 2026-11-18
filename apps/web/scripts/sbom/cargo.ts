import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { parse } from 'smol-toml';
import { identifyLicense, licenseFiles, type Component, type Product } from './bom.ts';
import { vendoredComponents } from './vendor.ts';

interface CargoPackage {
  name: string;
  version: string;
  source?: string;
}

export interface CargoTarget {
  package: string;
  target: string;
  manifest?: string;
  features?: string[];
  noDefaultFeatures?: boolean;
  product: Product;
}

export function cargoComponents(root: string, builds: CargoTarget[]): Component[] {
  const home = resolve(root, process.env.CARGO_HOME ?? join(homedir(), '.cargo'));
  const registry = join(home, 'registry', 'src');
  const registries = readdirSync(registry).map((directory) => join(registry, directory));
  const result: Component[] = [];
  for (const build of builds) {
    const manifest = build.manifest ?? 'Cargo.toml';
    const lock = join(root, dirname(manifest), 'Cargo.lock');
    const packageTable = parse(readFileSync(lock, 'utf8')).package;
    if (!Array.isArray(packageTable)) throw Error(`No package entries in ${lock}`);
    const packages: CargoPackage[] = packageTable.map((pkg) => {
      if (
        !pkg ||
        typeof pkg !== 'object' ||
        !('name' in pkg) ||
        typeof pkg.name !== 'string' ||
        !('version' in pkg) ||
        typeof pkg.version !== 'string'
      )
        throw Error(`Malformed package in ${lock}`);
      const source = 'source' in pkg && typeof pkg.source === 'string' ? pkg.source : undefined;
      return { name: pkg.name, version: pkg.version, source };
    });
    const common = ['--locked', '--offline', '--manifest-path', manifest];
    const cargo = (args: string[]) =>
      execFileSync('cargo', args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
    const tree = cargo([
      'tree',
      ...common,
      '-p',
      build.package,
      '--target',
      build.target,
      '--edges',
      'normal,no-proc-macro',
      '--prefix',
      'none',
      '--format',
      '{p}',
      ...(build.noDefaultFeatures ? ['--no-default-features'] : []),
      ...(build.features?.length ? ['--features', build.features.join(',')] : []),
    ]);
    let gitPackages: (CargoPackage & { manifest_path: string })[] | undefined;
    for (const line of new Set(
      tree
        .trim()
        .split('\n')
        .map((line) => line.replace(/ \(\*\)$/, '')),
    )) {
      const match = /^(\S+) v(\S+?)(?: \((.*)\))?$/.exec(line);
      if (!match) throw Error(`Unrecognized cargo tree entry: ${line}`);
      const [, name, version, location] = match;
      const key = `${name}@${version}`;
      if (location && isAbsolute(location)) {
        if (relative(root, location).startsWith('..'))
          throw Error(`${key}: external path crate needs pinned upstream provenance`);
        result.push(...vendoredComponents(root, location, build.product));
        continue;
      }
      let directory: string;
      let source: string;
      let purl: string | undefined;
      if (location) {
        // Cargo reports Git URLs with an abbreviated revision in tree output.
        // Resolve the full revision and checkout location with Cargo itself.
        gitPackages ??= (
          JSON.parse(
            cargo([
              'metadata',
              ...common,
              '--format-version',
              '1',
              '--filter-platform',
              build.target,
            ]),
          ) as { packages: (CargoPackage & { manifest_path: string })[] }
        ).packages;
        const [repository, commit] = location.split('#');
        const pkg = gitPackages.find(
          (pkg) =>
            pkg.name === name &&
            pkg.version === version &&
            pkg.source?.startsWith(`git+${repository}`) &&
            pkg.source.split('#')[1]?.startsWith(commit),
        );
        const revision = pkg?.source?.split('#')[1];
        if (!pkg || !revision)
          throw Error(`${key}: unable to resolve Cargo Git source ${location}`);
        directory = dirname(pkg.manifest_path);
        source = `${repository.split('?')[0].replace(/\.git$/, '')}/tree/${revision}`;
      } else {
        const sources = packages.filter(
          (pkg) =>
            pkg.name === name && pkg.version === version && pkg.source?.startsWith('registry+'),
        );
        const installed = registries
          .map((registry) => join(registry, `${name}-${version}`))
          .filter((path) => existsSync(join(path, 'Cargo.toml')));
        if (
          sources.length !== 1 ||
          installed.length !== 1 ||
          sources[0].source !== 'registry+https://github.com/rust-lang/crates.io-index'
        )
          throw Error(`${key}: cannot identify the installed Cargo.lock package source`);
        directory = installed[0];
        source = `https://static.crates.io/crates/${name}/${name}-${version}.crate`;
        purl = `pkg:cargo/${name}@${version}`;
      }
      const pkg = (
        parse(readFileSync(join(directory, 'Cargo.toml'), 'utf8')) as {
          package: { license?: string; 'license-file'?: string };
        }
      ).package;
      const texts = licenseFiles(
        directory,
        key,
        pkg['license-file'],
        join(root, 'apps/web/third_party/package-licenses/cargo', name, version),
      );
      result.push({
        name,
        version,
        source,
        purl,
        license: pkg.license ?? identifyLicense(texts[0], key),
        texts,
        products: [build.product],
      });
    }
  }
  return result;
}
