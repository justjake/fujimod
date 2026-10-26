/** Reject maintained JavaScript sources and TypeScript configurations that admit them. */
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
const compiler = fileURLToPath(new URL('../../node_modules/typescript/bin/tsc', import.meta.url));

export function violations(root: string, archives: string[] = []): string[] {
  const paths = execFileSync(
    'git',
    ['ls-files', '--cached', '--others', '--exclude-standard', '-z'],
    {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    },
  )
    .split('\0')
    .filter(Boolean);
  const errors: string[] = [];
  for (const path of new Set(paths)) {
    if (archives.some((prefix) => path.startsWith(prefix))) continue;
    const configuration = /\.(?:jsonc?|toml|ya?ml)$/i.test(path);
    const shell =
      /\.(?:sh|bash|zsh|fish)$/i.test(path) ||
      /(?:^|\/)(?:Makefile|Dockerfile)(?:\.[^/]*)?$/.test(path);
    const extensionless = /(?:^|\/)[^/.]+$/.test(path);
    if (!configuration && !shell && !extensionless && !/\.(?:[cm]?js|jsx|html?)$/i.test(path))
      continue;
    let source: string;
    try {
      source = readFileSync(resolve(root, path), 'utf8');
    } catch (error) {
      // Deleted tracked files and submodule directories are not source files.
      if (
        error instanceof Error &&
        'code' in error &&
        ['ENOENT', 'EISDIR'].includes(String(error.code))
      )
        continue;
      throw error;
    }
    if (/\.(?:[cm]?js|jsx)$/i.test(path) || /^#![^\n]*\bnode(?:\s|$)/.test(source))
      errors.push(`${path}: JavaScript source is forbidden; use TypeScript`);
    if (/\.html?$/i.test(path)) {
      for (const script of source.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)) {
        const attributes = script[1] ?? '',
          body = script[2]?.trim();
        const attribute = /\btype\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/i.exec(attributes);
        const type = (attribute?.[1] ?? attribute?.[2] ?? attribute?.[3])?.toLowerCase();
        if (body && !['importmap', 'application/json', 'application/ld+json'].includes(type ?? ''))
          errors.push(
            `${path}: inline executable script is forbidden; build a TypeScript entry point`,
          );
      }
    }
    // Configurations passed with --project may have any filename. Also inspect
    // maintained bases, rather than checking only tsconfig-prefixed children.
    const typescriptConfig =
      /(?:^|\/)(?:ts|js)config[^/]*\.jsonc?$/i.test(path) ||
      (/\.jsonc?$/i.test(path) && /"(?:compilerOptions|extends)"\s*:/.test(source));
    if (typescriptConfig) {
      const filename = resolve(root, path);
      const result = spawnSync(process.execPath, [compiler, '--showConfig', '-p', filename], {
        encoding: 'utf8',
      });
      if (result.error) throw result.error;
      if (result.status !== 0) {
        errors.push(
          `${path}: cannot resolve TypeScript configuration: ${result.stdout.trim()} ${result.stderr.trim()}`,
        );
        continue;
      }
      const parsed = JSON.parse(result.stdout) as { compilerOptions?: { allowJs?: boolean } };
      if (parsed.compilerOptions?.allowJs !== false)
        errors.push(`${path}: effective compilerOptions.allowJs must be explicitly false`);
    }
    if (configuration || shell || /^#![^\n]*\b(?:ba|z|k|a)?sh\b/.test(source)) {
      for (const override of source.matchAll(/--allowJs\b(?:[=\s]+(true|false|1|0))?/gi)) {
        if (override[1]?.toLowerCase() !== 'false') {
          errors.push(`${path}: command-line --allowJs is forbidden`);
          break;
        }
      }
    }
  }
  return errors;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const rootIndex = args.indexOf('--root');
  const root =
    rootIndex < 0
      ? resolve(dirname(fileURLToPath(import.meta.url)), '../..')
      : resolve(args[rootIndex + 1] ?? '.');
  const archives = args.flatMap((arg, index) =>
    arg === '--archive-prefix' ? [args[index + 1] ?? ''] : [],
  );
  const errors = violations(root, archives);
  if (errors.length) {
    console.error(errors.join('\n'));
    process.exitCode = 1;
  } else console.log(`TypeScript source policy passed (${relative(process.cwd(), root) || '.'})`);
}
