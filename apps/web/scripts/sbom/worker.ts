import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { emittedPackageDirectories } from './npm.ts';

type Metafile = {
  outputs: Record<string, { inputs: Record<string, { bytesInOutput: number }> }>;
};

export function workerPackages(root: string, output: string) {
  const web = join(root, 'apps/web');
  const directory = mkdtempSync(join(output, 'server', 'worker-bundle-'));
  const metafile = join(output, 'server', '.sbom-worker-metafile.json');
  try {
    execFileSync(
      join(root, 'apps/usb-sharing/node_modules/.bin/wrangler'),
      ['deploy', '--dry-run', '--outdir', directory, '--metafile', metafile],
      { cwd: web, stdio: 'inherit' },
    );
    const metadata: Metafile = JSON.parse(readFileSync(metafile, 'utf8'));
    const inputs = Object.values(metadata.outputs).flatMap((entry) =>
      Object.entries(entry.inputs)
        .filter(([, input]) => input.bytesInOutput > 0)
        .map(([path]) => path),
    );
    if (!inputs.length) throw Error('Missing emitted Worker modules in the bundle metafile');
    return emittedPackageDirectories(inputs, web);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}
