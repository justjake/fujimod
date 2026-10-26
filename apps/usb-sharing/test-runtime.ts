import { Miniflare } from 'miniflare';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// Test files run in parallel; each bundles the Worker into its own directory.
const outdir = mkdtempSync(join(tmpdir(), 'fujimod-sharing-worker-'));
execFileSync(
  fileURLToPath(new URL('./node_modules/.bin/wrangler', import.meta.url)),
  ['deploy', '--dry-run', '--outdir', outdir],
  { cwd: fileURLToPath(new URL('.', import.meta.url)), stdio: 'pipe' },
);
const worker = readFileSync(join(outdir, 'worker.js'), 'utf8');
rmSync(outdir, { recursive: true });
const origin = 'https://justjake.github.io';
/** A local Worker with the D1 index schema applied, as `wrangler d1 migrations apply` does. */
export async function runtime(
  options: {
    bindings?: Record<string, string>;
    outboundService?: (request: import('miniflare').Request) => Promise<Response>;
  } = {},
) {
  const mf = new Miniflare({
    workers: [
      {
        config: {
          name: 'sharing',
          compatibilityDate: '2026-10-17',
          manifest: {
            mainModule: 'worker.mjs',
            modules: {
              'worker.mjs': {
                type: 'esm',
                contents: worker,
              },
            },
          },
          exports: {
            Contributions: { type: 'durable-object', storage: 'sqlite' },
            IssueReports: { type: 'durable-object', storage: 'sqlite' },
            ShareRoom: { type: 'durable-object', storage: 'sqlite' },
            TurnBudget: { type: 'durable-object', storage: 'sqlite' },
          },
          env: {
            CONTRIBUTIONS: { type: 'durable-object', worker: 'sharing', exportName: 'Contributions' },
            CONTRIBUTION_LIMIT: { type: 'rate-limit', namespace: '1004', simple: { limit: 60, period: 60 } },
            ISSUES: { type: 'durable-object', worker: 'sharing', exportName: 'IssueReports' },
            ISSUE_REPORTS: { type: 'r2', name: 'reports' },
            ISSUE_LIMIT: {
              type: 'rate-limit',
              namespace: '1002',
              simple: { limit: 5, period: 60 },
            },
            ERROR_LIMIT: {
              type: 'rate-limit',
              namespace: '1003',
              simple: { limit: 30, period: 60 },
            },
            REPORTS_INDEX: { type: 'd1', name: 'reports' },
            ROOMS: { type: 'durable-object', worker: 'sharing', exportName: 'ShareRoom' },
            TURN_BUDGET: { type: 'durable-object', worker: 'sharing', exportName: 'TurnBudget' },
            CONNECTIONS: {
              type: 'rate-limit',
              namespace: '1001',
              simple: { limit: 60, period: 60 },
            },
            ...Object.fromEntries(
              Object.entries({ ALLOWED_ORIGINS: origin, ...options.bindings }).map(
                ([key, value]) => [key, { type: 'json', value }],
              ),
            ),
          },
        },
        dev: options.outboundService
          ? { outboundService: { type: 'fetcher', handler: options.outboundService } }
          : {},
      },
    ],
  });
  const index = await mf.getD1Database('REPORTS_INDEX');
  for (const migration of readdirSync(new URL('./migrations', import.meta.url)).sort())
    for (const statement of readFileSync(new URL(`./migrations/${migration}`, import.meta.url), 'utf8')
      .replace(/^--.*$/gm, '')
      .split(';')
      .map((text) => text.trim())
      .filter(Boolean))
      await index.prepare(statement).run();
  return mf;
}
