import { existsSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { type Plugin } from 'vite';
import { staticFiles } from '../local-review-files.ts';

export function previewLicenseFiles(): Plugin {
  return {
    name: 'credits-license-files',
    configureServer(server) {
      const directory = resolve(server.config.root, 'dist/client/licenses');
      server.middlewares.use((request, response, next) => {
        if (!request.url?.startsWith('/licenses/') || !existsSync(directory)) return next();
        return staticFiles(directory, '/licenses/')(request, response, next);
      });
    },
  };
}

/** Record installed packages that contribute output, including build-time CSS imports. */
export function sbomInputs(): Plugin {
  return {
    name: 'fujimod-sbom-inputs',
    apply: 'build',
    writeBundle(options, bundle) {
      if (!options.dir) throw Error('SBOM discovery requires a web output directory');
      const files = Object.values(bundle).flatMap((entry) =>
        entry.type === 'chunk' ? entry.moduleIds : entry.originalFileNames,
      );
      const packages = new Set<string>();
      for (const id of files) {
        const path = resolve(id.split('?')[0]);
        if (!path.includes('/node_modules/') || !existsSync(path)) continue;
        let directory = dirname(realpathSync(path));
        while (directory.includes('/node_modules/')) {
          const manifest = join(directory, 'package.json');
          if (existsSync(manifest)) {
            const pkg = JSON.parse(readFileSync(manifest, 'utf8')) as {
              name?: string;
              version?: string;
            };
            if (pkg.name && pkg.version) {
              packages.add(directory);
              break;
            }
          }
          directory = dirname(directory);
        }
      }
      writeFileSync(
        join(options.dir, '.sbom-npm-inputs.json'),
        JSON.stringify([...packages].sort()),
      );
    },
  };
}
