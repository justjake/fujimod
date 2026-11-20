/** Run the web server with the local sharing Worker behind it.
 *
 * Production serves this Worker at one configured origin. The preview runs
 * apps/usb-sharing with wrangler dev and proxies its public paths, so the
 * browser talks to the Worker through the page's own origin. */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { SHARING_WORKER_ORIGIN } from './sharing-worker.ts';

const [mode, ...args] = process.argv.slice(2);
if (mode !== 'dev' && mode !== 'preview')
  throw Error('Usage: node scripts/serve.ts <dev|preview> [Vite options]');
const web = resolve(import.meta.dirname, '..');
const worker = resolve(web, '../usb-sharing');

async function healthy(): Promise<boolean> {
  try {
    return (await fetch(`${SHARING_WORKER_ORIGIN}/health`, { signal: AbortSignal.timeout(1000) }))
      .ok;
  } catch {
    return false;
  }
}

const wrangler = resolve(worker, 'node_modules/wrangler/bin/wrangler.js');
if (!existsSync(wrangler))
  throw Error(
    'Install the USB sharing Worker dependencies with pnpm install --frozen-lockfile in fujimod',
  );

let workerProcess: ChildProcess | undefined;
if (!(await healthy())) {
  // wrangler dev creates the local D1 database but applies no schema.
  const migrate = spawnSync(
    process.execPath,
    [wrangler, 'd1', 'migrations', 'apply', 'REPORTS_INDEX', '--local'],
    { cwd: worker, stdio: 'inherit' },
  );
  if (migrate.status) throw Error('The sharing Worker database migrations failed');
  workerProcess = spawn(
    process.execPath,
    [
      wrangler,
      'dev',
      '--ip',
      '127.0.0.1',
      '--port',
      new URL(SHARING_WORKER_ORIGIN).port,
      // Keep the request URL local rather than the production custom-domain route.
      '--infer-origin-from-routes',
      'false',
      // The preview proxy supplies this Origin on the Worker's public routes.
      '--var',
      `ALLOWED_ORIGINS:${SHARING_WORKER_ORIGIN}`,
      // Share links stay relative so the browser resolves them to this origin.
      '--var',
      'SHARING_ORIGIN:',
    ],
    { cwd: worker, stdio: 'inherit' },
  );
  let exited = false;
  workerProcess.on('exit', (code) => {
    exited = true;
    workerProcess = undefined;
    if (code) console.error(`Sharing Worker exited with code ${code}`);
  });
  for (let attempt = 0; attempt < 60 && !exited && !(await healthy()); attempt++) await delay(500);
  if (!exited && !(await healthy()))
    console.error(
      `Sharing Worker did not answer at ${SHARING_WORKER_ORIGIN}; sharing routes will fail`,
    );
}

const server = spawn(
  process.execPath,
  [resolve(web, 'node_modules/vite-plus/bin/vp'), mode, ...args],
  { cwd: web, stdio: 'inherit' },
);
function shutdown(code: number) {
  workerProcess?.kill('SIGTERM');
  server.kill('SIGTERM');
  process.exit(code);
}
server.on('exit', (code) => shutdown(code ?? 0));
process.on('SIGTERM', () => shutdown(0));
process.on('SIGINT', () => shutdown(0));
