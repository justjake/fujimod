/** Build and upload the site and generated assets directly to Cloudflare. */
import { execFileSync } from 'node:child_process';
import { parseArgs } from 'node:util';
import { resolve } from 'node:path';
import { stageSite } from './stage-site.ts';
const { values } = parseArgs({
  options: {
    runtime: { type: 'string' },
    media: { type: 'string' },
    'prepare-only': { type: 'boolean', default: false },
  },
});
if (!values.runtime || !values.media) throw Error('Provide --runtime DIRECTORY --media DIRECTORY.');
console.log(await stageSite({ runtime: values.runtime, media: values.media }));
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
  run('npm', ['run', 'check']);
  run('npm', ['test']);
  run('npm', ['run', 'build']);
  run('npm', ['run', 'test:prerender']);
  run(process.execPath, ['../../scripts/cloudflare/wrangler.ts', 'deploy']);
}
