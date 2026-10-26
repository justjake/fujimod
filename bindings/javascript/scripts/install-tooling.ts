// Install the official prebuilt CLI matching this workspace's wasm-bindgen ABI.
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, copyFile, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
const root = new URL('../../../', import.meta.url);
const lock = await readFile(new URL('Cargo.lock', root), 'utf8');
const version = lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
if (!version) throw Error('No wasm-bindgen version in Cargo.lock');
const targets: Record<string, string> = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-musl',
  'linux-arm64': 'aarch64-unknown-linux-musl',
  'win32-x64': 'x86_64-pc-windows-msvc',
};
const target = targets[`${process.platform}-${process.arch}`];
if (!target) throw Error('Install wasm-bindgen-cli with cargo for this host.');
const name = `wasm-bindgen-${version}-${target}`;
const base = `https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}/${name}.tar.gz`;
const responses = await Promise.all([fetch(base), fetch(`${base}.sha256sum`)]);
for (const response of responses)
  if (!response.ok) throw Error(`${response.url}: HTTP ${response.status}`);
const data = Buffer.from(await responses[0].arrayBuffer());
const expected = (await responses[1].text()).trim().split(/\s+/)[0];
if (createHash('sha256').update(data).digest('hex') !== expected)
  throw Error('wasm-bindgen CLI checksum mismatch');
const directory = await mkdtemp(join(tmpdir(), 'fujimod-bindgen-'));
try {
  const { writeFile } = await import('node:fs/promises');
  await writeFile(join(directory, 'cli.tar.gz'), data);
  execFileSync('tar', ['-xzf', join(directory, 'cli.tar.gz'), '-C', directory]);
  const bin = fileURLToPath(new URL('target/tooling/bin/', root));
  await mkdir(bin, { recursive: true });
  for (const tool of ['wasm-bindgen', 'wasm-bindgen-test-runner']) {
    const executable = process.platform === 'win32' ? `${tool}.exe` : tool;
    await copyFile(join(directory, name, executable), join(bin, executable));
    await chmod(join(bin, executable), 0o755);
  }
  console.log(`Installed wasm-bindgen ${version}`);
} finally {
  await rm(directory, { recursive: true, force: true });
}
