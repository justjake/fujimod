import { expect, test } from 'vite-plus/test';
import { Wizard, isDevToolsPath, toolAt, toolPath } from './routes';

test('each tool has its own path and the workbench has none', () => {
  for (const tool of Object.values(Wizard)) {
    expect(toolAt(toolPath(tool))).toBe(tool);
    expect(isDevToolsPath(toolPath(tool))).toBe(true);
  }
  expect(toolPath(undefined)).toBe('/dev-tools/');
  expect(toolAt('/dev-tools/')).toBeUndefined();
  expect(toolAt('/dev-tools/state-captures')).toBe('state-captures');
  expect(isDevToolsPath('/dev-tools/unknown/')).toBe(false);
  expect(isDevToolsPath('/bug/')).toBe(false);
});
