import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync, mkdirSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { violations } from './check-typescript.ts';

function fixture(run: (root: string, put: (path: string, body: string) => void) => void) {
  const root = mkdtempSync(join(tmpdir(), 'typescript-policy-'));
  const put = (path: string, body: string) => {
    const filename = join(root, path);
    mkdirSync(dirname(filename), { recursive: true });
    writeFileSync(filename, body);
  };
  try {
    execFileSync('git', ['init', '--quiet', root]);
    put('entry.ts', 'export const value: number = 1;');
    put(
      'tsconfig.json',
      JSON.stringify({ compilerOptions: { allowJs: false }, include: ['*.ts'] }),
    );
    run(root, put);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

await test('checks newly added JavaScript and inline scripts, while allowing data and generated output', () =>
  fixture((root, put) => {
    put('.gitignore', 'build/\n');
    mkdirSync(join(root, 'build'));
    put('build/generated.js', 'generated();');
    put(
      'page.html',
      '<script type=importmap>{"imports":{}}</script><script type=application/json>{}</script><script type="application/ld+json">{}</script><script src="/entry.js"></script>',
    );
    assert.deepEqual(violations(root), []);
    put('new.mjs', 'export const value = 1;');
    put('javascript-tool', '#!/usr/bin/env node\nrun();');
    put('page.html', '<script type="module">run()</script>');
    const errors = violations(root);
    assert.ok(errors.some((error) => error.includes('new.mjs: JavaScript')));
    assert.ok(errors.some((error) => error.includes('javascript-tool: JavaScript')));
    assert.ok(errors.some((error) => error.includes('page.html: inline')));
  }));

await test('rejects inherited allowJs and a missing explicit false', () =>
  fixture((root, put) => {
    put('.gitignore', 'node_modules/\n');
    put('node_modules/external/base.json', JSON.stringify({ compilerOptions: { allowJs: true } }));
    put(
      'tsconfig.json',
      JSON.stringify({ extends: './node_modules/external/base.json', include: ['*.ts'] }),
    );
    assert.ok(violations(root).some((error) => error.includes('allowJs must be explicitly false')));
    put(
      'tsconfig.json',
      JSON.stringify({
        extends: './node_modules/external/base.json',
        compilerOptions: { allowJs: false },
        include: ['*.ts'],
      }),
    );
    assert.deepEqual(violations(root), []);
    put('tsconfig.json', JSON.stringify({ include: ['*.ts'] }));
    assert.ok(violations(root).some((error) => error.includes('allowJs must be explicitly false')));
  }));

await test('rejects command-line overrides', () =>
  fixture((root, put) => {
    put('package.json', JSON.stringify({ scripts: { check: 'tsc --allowJs' } }));
    assert.ok(violations(root).some((error) => error.includes('command-line --allowJs')));
  }));

await test('checks custom project filenames, JSONC and inherited custom bases', () =>
  fixture((root, put) => {
    put(
      'browser.project.jsonc',
      '{ /* project */ "compilerOptions": { "allowJs": true }, "include": ["*.ts"] }',
    );
    assert.ok(
      violations(root).some(
        (error) => error.startsWith('browser.project.jsonc:') && error.includes('allowJs'),
      ),
    );
    put('browser.project.jsonc', '{ "extends": "./settings.json", "include": ["*.ts"] }');
    put('settings.json', JSON.stringify({ compilerOptions: { allowJs: true }, include: ['*.ts'] }));
    const errors = violations(root);
    assert.ok(
      errors.some((error) => error.startsWith('settings.json:') && error.includes('allowJs')),
    );
    assert.ok(
      errors.some(
        (error) => error.startsWith('browser.project.jsonc:') && error.includes('allowJs'),
      ),
    );
    put(
      'settings.json',
      JSON.stringify({ compilerOptions: { allowJs: false }, include: ['*.ts'] }),
    );
    assert.deepEqual(violations(root), []);
  }));

await test('rejects shell and task overrides, including lower-case flags and later overrides', () =>
  fixture((root, put) => {
    for (const [path, body] of [
      ['check.sh', 'tsc --allowJs'],
      ['build', '#!/bin/sh\nexec tsc --allowjs true'],
      ['Makefile', 'check:\n\ttsc --allowJs false --allowJs true'],
      ['tasks.json', JSON.stringify({ command: 'tsc --allowJs=true' })],
      ['tasks.toml', '[tasks.check]\nrun = "tsc --allowJs 1"'],
      ['workflow.yml', 'run: tsc --ALLOWJS'],
    ])
      put(path!, body!);
    const errors = violations(root);
    for (const path of [
      'check.sh',
      'build',
      'Makefile',
      'tasks.json',
      'tasks.toml',
      'workflow.yml',
    ])
      assert.ok(
        errors.some(
          (error) => error.startsWith(`${path}:`) && error.includes('command-line --allowJs'),
        ),
        path,
      );
  }));

await test('accepts explicit command-line disabling of JavaScript', () =>
  fixture((root, put) => {
    put('check.sh', 'tsc --allowJs false');
    put('package.json', JSON.stringify({ scripts: { check: 'tsc --allowJs false' } }));
    assert.deepEqual(violations(root), []);
  }));

await test('ignores deleted tracked sources and fails on unexpected read errors', () =>
  fixture((root, put) => {
    put('deleted.js', 'old();');
    execFileSync('git', ['add', '--', 'deleted.js'], { cwd: root });
    rmSync(join(root, 'deleted.js'));
    assert.deepEqual(violations(root), []);
    symlinkSync('unreadable.json', join(root, 'unreadable.json'));
    assert.throws(
      () => violations(root),
      (error: unknown) => error instanceof Error && 'code' in error && error.code === 'ELOOP',
    );
  }));
