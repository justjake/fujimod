import { test, expect } from 'vite-plus/test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

for (const mode of ['production', 'local-preview']) {
  test(`${mode} bundle ${mode === 'production' ? 'eliminates' : 'includes'} automatic sharing`, async () => {
    const { stdout } = await promisify(execFile)(
      process.execPath,
      [fileURLToPath(new URL('./dev-share-build-fixture.ts', import.meta.url)), mode],
      {
        env: { ...process.env, NODE_ENV: 'production' },
      },
    );
    const { code, modules } = JSON.parse(stdout);
    const included = mode === 'local-preview';
    expect(modules.some((id: string) => id.endsWith('/dev-share.ts'))).toBe(included);
    expect(code.includes('/usb-proxy/dev-share')).toBe(included);
    expect(code.includes('Development share registered')).toBe(included);
  });
}
