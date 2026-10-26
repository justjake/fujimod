/** Build and upload the site and generated assets directly to Cloudflare. */
import { execFileSync } from 'node:child_process';
import { parseArgs } from 'node:util';
import { resolve } from 'node:path';
import { stageSite } from './stage-site.ts';
const { values } = parseArgs({
  options: {
    runtime: { type: 'string' },
    patches: { type: 'string' },
    media: { type: 'string' },
    'prepare-only': { type: 'boolean', default: false },
  },
});
if (!values.runtime || !values.patches || !values.media)
  throw Error('Provide --runtime DIRECTORY --patches DIRECTORY --media DIRECTORY.');
console.log(
  await stageSite({ runtime: values.runtime, patches: values.patches, media: values.media }),
);
if (!values['prepare-only']) {
  const run = (command: string, args: string[]) =>
    execFileSync(command, args, {
      stdio: 'inherit',
      env: {
        ...process.env,
        FUJIMOD_RUNTIME: resolve('public/runtime'),
        FUJIMOD_PATCHES: resolve('public/artifacts'),
        FUJIMOD_MEDIA: resolve('public/artifacts/docs'),
      },
    });
  run('npm', ['run', 'check']);
  run('npm', ['test']);
  run('npm', ['run', 'build']);
  run(process.execPath, ['../../scripts/cloudflare/wrangler.ts', 'deploy']);
}
