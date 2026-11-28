/** Build and upload the site and generated assets directly to Cloudflare. */
import { execFileSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { resolve } from 'node:path';
import { carrySite } from './carry-site.ts';
import { filmEditorDemoPath } from './film-editor-demo.ts';
import { stageSite } from './stage-site.ts';
import { sdkDocsManifest, sdkDocsPath } from './sdk-docs.ts';
// pnpm forwards the optional argument separator to the script.
const args = process.argv.slice(process.argv[2] === '--' ? 3 : 2);
const { values } = parseArgs({
  args,
  options: {
    runtime: { type: 'string' },
    media: { type: 'string' },
    'sdk-docs': { type: 'string' },
    'carry-from': { type: 'string' },
    'film-editor-demo': { type: 'string' },
    'prepare-only': { type: 'boolean', default: false },
  },
});
const {
  runtime,
  media,
  'sdk-docs': sdkDocs,
  'carry-from': origin,
  'film-editor-demo': filmEditorDemo,
} = values;
const usage =
  'Provide either --runtime DIRECTORY --media DIRECTORY --sdk-docs DIRECTORY [--film-editor-demo DIRECTORY] or --carry-from ORIGIN.';
if (origin) {
  if (runtime || media || sdkDocs || filmEditorDemo) throw Error(usage);
  console.log(await carrySite(origin));
} else {
  if (!runtime || !media || !sdkDocs) throw Error(usage);
  console.log(await stageSite({ runtime, media, sdkDocs, filmEditorDemo }));
}
if (!values['prepare-only']) {
  const stagedSdkDocs = resolve('public', sdkDocsPath);
  const stagedFilmEditorDemo = resolve('public', filmEditorDemoPath);
  const run = (command: string, args: string[]) =>
    execFileSync(command, args, {
      stdio: 'inherit',
      env: {
        ...process.env,
        FUJIMOD_RUNTIME: resolve('public/runtime'),
        FUJIMOD_MEDIA: resolve('public/artifacts/docs'),
        FUJIMOD_SDK_DOCS: existsSync(resolve(stagedSdkDocs, sdkDocsManifest))
          ? stagedSdkDocs
          : undefined,
        FUJIMOD_FILM_EDITOR_DEMO: existsSync(stagedFilmEditorDemo)
          ? stagedFilmEditorDemo
          : undefined,
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
