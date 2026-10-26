/** Run the sharing Worker's pinned Wrangler. Without CLOUDFLARE_API_TOKEN, it uses
 * the signed-in Cloudflare CLI (`cf auth login`) OAuth token, so Wrangler needs no
 * separate login or API token. */
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const wrangler = fileURLToPath(
  new URL('../../apps/usb-sharing/node_modules/.bin/wrangler', import.meta.url),
);

function cfToken(): string | undefined {
  const home = homedir();
  const config = [
    join(home, 'Library/Preferences/cloudflare/config/default.json'),
    join(process.env.XDG_CONFIG_HOME ?? join(home, '.config'), 'cloudflare/config/default.json'),
  ].find(existsSync);
  if (!config) return undefined;
  // Refreshes an expired OAuth token before it is read.
  execFileSync('cf', ['auth', 'whoami'], { stdio: 'ignore' });
  const { oauth_token: token } = JSON.parse(readFileSync(config, 'utf8')) as {
    oauth_token?: string;
  };
  return token;
}

const env: NodeJS.ProcessEnv = { ...process.env, WRANGLER_SEND_METRICS: 'false' };
env.CLOUDFLARE_API_TOKEN ||= cfToken();
if (!env.CLOUDFLARE_API_TOKEN)
  throw Error('Sign in with `cf auth login` or set CLOUDFLARE_API_TOKEN.');
const result = spawnSync(wrangler, process.argv.slice(2), { stdio: 'inherit', env });
process.exit(result.status ?? 1);
