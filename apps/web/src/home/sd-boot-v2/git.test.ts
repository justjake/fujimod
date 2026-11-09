import { expect, test } from 'vite-plus/test';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { gitRepository } from './git';

test('the written repository passes git fsck, has one commit and a clean work tree', async () => {
  const encode = (text: string) => new TextEncoder().encode(text);
  const files = [
    { path: 'RECIPES/AGFAOPT.JSN', data: encode('{"name":"Agfa Optima"}\n') },
    { path: 'RECIPES-A.JSN', data: encode('ordering\n') },
    { path: 'SYSTEM/X70-110/A.JSN', data: encode('{"gen":1}\n') },
    { path: 'SYSTEM/STORE/E3B0C442.98F', data: new Uint8Array() },
    { path: 'SETTINGS.JSN', data: encode('{}\n') },
  ];
  const directory = await mkdtemp(join(tmpdir(), 'sd-card-git-'));
  try {
    for (const file of files) {
      await mkdir(dirname(join(directory, file.path)), { recursive: true });
      await writeFile(join(directory, file.path), file.data);
    }
    const repository = await gitRepository(files, {
      name: 'FujiMod',
      email: 'noreply@fujimod.com',
      message: 'FujiMod 1.0',
      time: 1_800_000_000,
    });
    for (const file of repository) {
      await mkdir(dirname(join(directory, '.git', file.path)), { recursive: true });
      await writeFile(join(directory, '.git', file.path), file.data);
    }
    const git = (...args: string[]) =>
      execFileSync('git', ['-c', 'safe.directory=*', ...args], {
        cwd: directory,
        encoding: 'utf8',
        env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
      });
    git('fsck', '--strict', '--no-dangling');
    expect(git('log', '--format=%an <%ae> %at %s')).toBe(
      'FujiMod <noreply@fujimod.com> 1800000000 FujiMod 1.0\n',
    );
    expect(git('ls-tree', '-r', '--name-only', 'HEAD').trim().split('\n')).toEqual(
      files.map((file) => file.path).sort(),
    );
    expect(git('status', '--porcelain')).toBe('');
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('paths Git cannot track are refused', async () => {
  const commit = { name: 'a', email: 'a@b', message: 'm', time: 0 };
  for (const path of ['.git/config', 'A//B', '../A'])
    await expect(gitRepository([{ path, data: new Uint8Array() }], commit)).rejects.toThrow();
});
