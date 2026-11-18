import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

/** Materialize the SDK's packaged Wasm previews for dev and static hosting. */
export function prepareSdkPreviews(webRoot: string): void {
  const bundle = resolve(webRoot, 'sdk-previews.generated.tar.gz');
  if (!existsSync(bundle)) return;

  const destination = resolve(webRoot, 'public/docs/sdk/focal');
  const receipt = resolve(webRoot, 'build/sdk-previews.sha256');
  const bytes = readFileSync(bundle);
  const digest = createHash('sha256').update(bytes).digest('hex');
  if (
    existsSync(resolve(destination, 'manifest.json')) &&
    existsSync(receipt) &&
    readFileSync(receipt, 'utf8') === digest
  )
    return;

  rmSync(receipt, { force: true });
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });
  execFileSync('tar', ['-xzf', '-', '-C', destination], { input: bytes });
  mkdirSync(resolve(webRoot, 'build'), { recursive: true });
  writeFileSync(receipt, digest);
}
