/** Build and upload the site and generated assets directly to Cloudflare. */
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { resolve } from 'node:path';
import { carrySite } from './carry-site.ts';
import { stageSite } from './stage-site.ts';
const { values } = parseArgs({
  options: {
    runtime: { type: 'string' },
    media: { type: 'string' },
    'carry-from': { type: 'string' },
    'prepare-only': { type: 'boolean', default: false },
  },
});
const origin = values['carry-from'];
if (origin ? values.runtime || values.media : !values.runtime || !values.media)
  throw Error('Provide either --runtime DIRECTORY --media DIRECTORY or --carry-from ORIGIN.');
console.log(
  origin
    ? await carrySite(origin)
    : await stageSite({ runtime: values.runtime!, media: values.media! }),
);
if (!values['prepare-only']) {
  const run = (command: string, args: string[]) =>
    execFileSync(command, args, {
      stdio: 'inherit',
      env: {
        ...process.env,
        FUJIMOD_RUNTIME: resolve('public/runtime'),
        FUJIMOD_MEDIA: resolve('public/artifacts/docs'),
      },
    });
  run('pnpm', ['run', 'check']);
  run('pnpm', ['test']);
  run('pnpm', ['run', 'build']);
  run('pnpm', ['run', 'test:prerender']);
  const git = (...args: string[]) => execFileSync('git', args, { encoding: 'utf8' }).trim();
  const deployment = {
    commit: git('rev-parse', 'HEAD'),
    dirty: git('status', '--porcelain', '--untracked-files=no') !== '',
  };
  writeFileSync('dist/client/deployment.json', JSON.stringify(deployment) + '\n');
  run(process.execPath, ['../../scripts/cloudflare/wrangler.ts', 'deploy']);
}
