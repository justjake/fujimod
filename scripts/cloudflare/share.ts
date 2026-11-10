/** Publish a local file to the shared-files bucket and print its public URL. */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdtemp, realpath, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { MIMEType, parseArgs } from 'node:util';

const worker = fileURLToPath(new URL('../../apps/usb-sharing/', import.meta.url));
const wrapper = fileURLToPath(new URL('./wrangler.ts', import.meta.url));

/** True for C0 control characters and DEL, which no share path may contain. */
function hasControlCharacter(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}
export function shareUrl(key: string, direct = false): string {
  if (
    !key ||
    hasControlCharacter(key) ||
    key.includes('\\') ||
    key.split('/').some((part) => !part || part === '.' || part === '..')
  )
    throw Error(
      'The R2 key must contain nonempty path segments without traversal or control characters.',
    );
  return `https://share.fujimod.com/file/${key.split('/').map(encodeURIComponent).join('/')}${direct ? '?direct=true' : ''}`;
}

export async function publication(
  file: string,
  options: { contentType?: string; key?: string; direct?: boolean } = {},
) {
  const path = await realpath(resolve(file));
  if (!(await stat(path)).isFile()) throw Error('Choose a regular file.');
  const contentType =
    options.contentType ??
    execFileSync('file', ['--brief', '--mime-type', path], { encoding: 'utf8' }).trim();
  const mime = new MIMEType(contentType);
  const media = mime.type === 'image' || mime.type === 'video';
  if (options.direct && !media) throw Error('--direct requires an image or video MIME type.');
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  const key = options.key ?? `files/${hash.digest('hex')}/${basename(path)}`;
  return {
    path,
    key,
    contentType,
    video: mime.type === 'video',
    url: shareUrl(key, options.direct),
  };
}

function upload(path: string, key: string, contentType: string): void {
  execFileSync(
    process.execPath,
    [
      wrapper,
      'r2',
      'object',
      'put',
      `fujimod-shared-files/${key}`,
      '--remote',
      '--file',
      path,
      '--content-type',
      contentType,
    ],
    {
      cwd: worker,
      stdio: ['ignore', 2, 2],
    },
  );
}

export async function main(args: string[]): Promise<void> {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: {
      'content-type': { type: 'string' },
      key: { type: 'string' },
      poster: { type: 'string' },
      direct: { type: 'boolean' },
      help: { type: 'boolean' },
    },
  });
  if (values.help) {
    process.stdout.write(
      'mise run share -- FILE [--content-type MIME] [--key R2/PATH] [--poster PNG] [--direct]\n',
    );
    return;
  }
  if (positionals.length !== 1)
    throw Error(
      'Usage: mise run share -- FILE [--content-type MIME] [--key R2/PATH] [--poster PNG] [--direct]',
    );
  const plan = await publication(positionals[0], {
    contentType: values['content-type'],
    key: values.key,
    direct: values.direct,
  });
  if (values.poster && !plan.video) throw Error('--poster requires a video.');
  let temporary: string | undefined;
  try {
    if (plan.video) {
      let poster: string;
      if (values.poster) {
        const image = await publication(values.poster);
        if (new MIMEType(image.contentType).essence !== 'image/png')
          throw Error('The poster must be a PNG file.');
        poster = image.path;
      } else {
        temporary = await mkdtemp(join(tmpdir(), 'fujimod-share-'));
        poster = join(temporary, 'poster.png');
        execFileSync(
          'ffmpeg',
          ['-v', 'error', '-i', plan.path, '-frames:v', '1', '-update', '1', poster],
          { stdio: ['ignore', 2, 2] },
        );
      }
      upload(poster, `${plan.key}.poster.png`, 'image/png');
    }
    upload(plan.path, plan.key, plan.contentType);
    process.stdout.write(`${plan.url}\n`);
  } finally {
    if (temporary) await rm(temporary, { recursive: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch((error: unknown) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
