import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { basename, dirname } from 'node:path';

type CargoMessage = {
  reason: string;
  message?: { rendered?: string };
  manifest_path?: string;
  filenames?: string[];
};

/** The artifact stems tie linker entries to the exact Cargo package, including parallel versions. */
export function cargoArtifacts(
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
  rustcArgs: string[] = [],
) {
  const result = spawnSync(
    'cargo',
    [
      ...args,
      '--message-format=json-render-diagnostics',
      ...(rustcArgs.length ? ['--', ...rustcArgs] : []),
    ],
    {
      cwd,
      env,
      encoding: 'utf8',
      stdio: ['inherit', 'pipe', 'inherit'],
      maxBuffer: 32 * 1024 * 1024,
    },
  );
  if (result.error) throw result.error;
  const artifacts = new Map<string, string>();
  for (const line of result.stdout.split('\n')) {
    if (!line.startsWith('{')) continue;
    const message: CargoMessage = JSON.parse(line);
    if (message.reason === 'compiler-message' && message.message?.rendered)
      process.stderr.write(message.message.rendered);
    if (message.reason !== 'compiler-artifact' || !message.manifest_path) continue;
    for (const artifact of message.filenames ?? []) {
      const stem = /^lib([\w]+-[a-f0-9]+)\.rlib$/.exec(basename(artifact))?.[1];
      if (stem) artifacts.set(stem, dirname(message.manifest_path));
    }
  }
  if (result.status !== 0) throw Error(`Cargo exited ${result.status ?? result.signal}`);
  return artifacts;
}

export function linkedCrates(map: string, artifacts: ReadonlyMap<string, string>) {
  const directories = new Set<string>();
  // Nonzero linker-map entries cover object members of the emulator's static
  // archive and rlibs linked directly by rustc into standalone Wasm modules.
  const rows = /^ *[-\da-f]+ +[\da-f]+ +([\da-f]+) +([^\r\n]+)/gm;
  for (const [, size, input] of map.matchAll(rows)) {
    if (!Number.parseInt(size, 16)) continue;
    const stem =
      /(?:^|[/\\])lib([\w]+-[a-f0-9]+)\.rlib\(/.exec(input)?.[1] ??
      /\.a\(([\w]+-[a-f0-9]+)\.[^()]*\.rcgu\.o\)/.exec(input)?.[1] ??
      /[/\\]([\w]+-[a-f0-9]+)\.[^()]*\.rcgu\.o:/.exec(input)?.[1];
    const directory = stem && artifacts.get(stem);
    if (directory) directories.add(directory);
  }
  if (!directories.size)
    throw Error('No Cargo artifacts matched emitted bytes in the Wasm linker map');
  return [...directories].sort();
}

export function readLinkedCrates(map: string, artifacts: ReadonlyMap<string, string>) {
  return linkedCrates(readFileSync(map, 'utf8'), artifacts);
}
