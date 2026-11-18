import { existsSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { type Plugin } from 'vite';
import { staticFiles } from '../local-review-files.ts';
import { emittedPackageDirectories } from './npm.ts';

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
      if (this.environment.name !== 'client') return;
      if (!options.dir) throw Error('SBOM discovery requires a client output directory');
      const files = Object.values(bundle).flatMap((entry) =>
        entry.type === 'chunk'
          ? Object.entries(entry.modules)
              .filter(([, module]) => module.renderedLength > 0)
              .map(([id]) => id)
          : entry.originalFileNames,
      );
      writeFileSync(
        join(options.dir, '.sbom-npm-inputs.json'),
        JSON.stringify(emittedPackageDirectories(files, this.environment.config.root)),
      );
    },
  };
}
